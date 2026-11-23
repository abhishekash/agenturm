"""Qualify captured evaluation details through distinct broker roles in a Linux guest."""

import argparse
import codecs
import re
import unicodedata
import fcntl
import pty
import struct
import termios
import time
from contextlib import closing
import grp
import json
import os
from pathlib import Path
import pwd
import secrets
import select
import sqlite3
import stat
import subprocess
import tempfile

from validate_rust_remote_history import private_directory, request, request_get, stop, wait_for, write_token

SOURCE = 'otlp.eval-detail-fixture'
TRACE = '11' * 16
BODY_A = 'bounded evaluation error from first receiver'
BODY_B = 'bounded evaluation error from second receiver'
SCORE = 'bounded private scorer explanation'
DISCARDED = 'untyped provider output intentionally discarded'


class TerminalCells:
    """Reconstruct the fixture's CSI-based alternate screen across PTY reads."""

    CSI = re.compile(r"\x1b\[([0-?]*)([ -/]*)([@-~])")

    def __init__(self, columns, rows):
        self.columns, self.rows = columns, rows
        self.clear()
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")()
        self.departed = None

    def clear(self):
        self.cells = [[" "] * self.columns for _ in range(self.rows)]
        self.row = self.column = 0

    def contents(self):
        return "\n".join("".join(row) for row in self.cells)

    def apply(self, parameters, command):
        if parameters == "?1049":
            if command == "h":
                self.clear()
            elif command == "l":
                self.departed = self.contents()
            return
        if parameters.startswith(("?", ">", "=")) or command in "mhl":
            return
        values = [int(value or 0) for value in parameters.split(";")]
        first = values[0] or 1
        if command in "Hf":
            self.row = min(self.rows - 1, first - 1)
            self.column = min(self.columns - 1, (values[1] or 1) - 1 if len(values) > 1 else 0)
        elif command == "G":
            self.column = min(self.columns - 1, first - 1)
        elif command == "d":
            self.row = min(self.rows - 1, first - 1)
        elif command == "A":
            self.row = max(0, self.row - first)
        elif command == "B":
            self.row = min(self.rows - 1, self.row + first)
        elif command == "C":
            self.column = min(self.columns - 1, self.column + first)
        elif command == "D":
            self.column = max(0, self.column - first)
        elif command == "J":
            if values[0] in (2, 3):
                self.clear()
            elif values[0] == 0:
                self.cells[self.row][self.column:] = [" "] * (self.columns - self.column)
                self.cells[self.row + 1:] = [[" "] * self.columns for _ in range(self.rows - self.row - 1)]
            elif values[0] == 1:
                self.cells[:self.row] = [[" "] * self.columns for _ in range(self.row)]
                width = min(self.columns, self.column + 1)
                self.cells[self.row][:width] = [" "] * width
        elif command == "K":
            first, last = (self.column, self.columns) if values[0] == 0 else (0, min(self.columns, self.column + 1)) if values[0] == 1 else (0, self.columns)
            self.cells[self.row][first:last] = [" "] * (last - first)

    def feed(self, chunk):
        pending = self.pending + self.decoder.decode(chunk)
        index = 0
        while index < len(pending):
            char = pending[index]
            if char == "\x1b":
                if index + 1 == len(pending):
                    break
                if pending[index + 1] == "[":
                    match = self.CSI.match(pending, index)
                    if match is None:
                        break
                    self.apply(match[1], match[3])
                    index = match.end()
                    continue
                index += 2
                continue
            index += 1
            if char == "\r":
                self.column = 0
            elif char == "\n":
                self.row = min(self.rows - 1, self.row + 1)
            elif char == "\b":
                self.column = max(0, self.column - 1)
            elif char >= " ":
                if unicodedata.combining(char):
                    if self.column:
                        self.cells[self.row][self.column - 1] += char
                    continue
                if self.column >= self.columns:
                    self.column = 0
                    self.row = min(self.rows - 1, self.row + 1)
                self.cells[self.row][self.column] = char
                self.column += 1
                if unicodedata.east_asian_width(char) in ("F", "W") and self.column < self.columns:
                    self.cells[self.row][self.column] = " "
                    self.column += 1
        self.pending = pending[index:]


def cockpit_preview(receiver, state, reader, gid, admin, binding, columns, rows, close):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
    original = termios.tcgetattr(slave)
    env = {**os.environ, 'HOME': str(state), 'XDG_STATE_HOME': str(state / 'xdg'), 'TERM': 'xterm-256color'}
    child = subprocess.Popen(receiver.cmd('cockpit'), cwd=state, stdin=slave, stdout=slave, stderr=slave,
                             user=reader, group=gid, extra_groups=[], env=env)
    screen = TerminalCells(columns, rows)

    def wait_for_cells(predicate, phase):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if select.select([master], [], [], .05)[0]:
                screen.feed(os.read(master, 8192))
            if predicate():
                return screen.contents()
            assert child.poll() is None, (phase, screen.contents())
        raise AssertionError((phase, screen.contents()))

    def until(marker):
        return wait_for_cells(lambda: marker in screen.contents(), marker)

    def redacted(contents):
        return BODY_A not in contents and SCORE not in contents

    revoke = None
    try:
        assert redacted(until('eval-session'))
        os.write(master, b'8')
        assert redacted(until('E evaluations'))
        os.write(master, b'E')
        assert redacted(until('Enter evaluations'))
        os.write(master, b'\r')
        assert redacted(until('d private'))
        os.write(master, b'd')
        wait_for_cells(lambda: all(marker in screen.contents() for marker in
                      ('Imported evaluation text', BODY_A, SCORE)), 'both private texts')
        revoke = subprocess.Popen(receiver.cmd('grant', 'revoke', '--binding', binding),
                                  user=admin, group=gid, extra_groups=[], stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE)
        time.sleep(.25)
        assert revoke.poll() is None, revoke.stderr.read() if revoke.poll() is not None else ''
        os.write(master, close)
        if close == b'\x1b':
            wait_for_cells(lambda: 'd private' in screen.contents() and redacted(screen.contents()),
                           'return to redacted evaluation list')
            assert revoke.wait(timeout=8) == 0, revoke.stderr.read()
            os.write(master, b'q')
        wait_for_cells(lambda: screen.departed is not None, 'leave alternate screen')
        assert redacted(screen.departed), screen.departed
        assert revoke.wait(timeout=8) == 0, revoke.stderr.read()
        assert child.wait(timeout=8) == 0
        assert termios.tcgetattr(slave) == original
        print(f'PASS evaluation private display {columns}x{rows}, redaction, and terminal modes ({close.hex()})', flush=True)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=8)
        if revoke is not None and revoke.poll() is None:
            revoke.kill()
            revoke.wait(timeout=8)
        os.close(master)
        os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, required=True)
    args = parser.parse_args()
    assert os.geteuid() == 0
    used_uids = {entry.pw_uid for entry in pwd.getpwall()}
    used_gids = {entry.gr_gid for entry in grp.getgrall()}
    broker, collector, reader, admin, maintainer = [uid for uid in range(45000, 46000) if uid not in used_uids][:5]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in used_gids)
    app = args.bin_dir.resolve(strict=True) / 'agenturm'
    with tempfile.TemporaryDirectory(prefix='agenturm-private-eval-clitest-') as path:
        root = Path(path)
        root.chmod(0o755)
        token = secrets.token_hex(32)
        token_file = root / 'reader-token'
        write_token(token_file, token, reader, client_gid)

        def run(uid, argv, code=0, gid=client_gid):
            output = subprocess.run(argv, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    text=True, user=uid, group=gid, extra_groups=[], timeout=12)
            assert output.returncode == code, (argv[1:3], output.stderr)
            assert DISCARDED not in output.stdout and BODY_A not in output.stderr and BODY_B not in output.stderr
            if code:
                assert not output.stdout, argv[1:3]
            return output.stdout

        binding_file = root / 'grant.json'
        binding_file.write_text(run(reader, [app, 'eval-detail', 'binding', '--source-id', SOURCE]))
        selection_file = root / 'selection.json'

        class Receiver:
            def __init__(self, label):
                self.label = label
                self.path = root / label
                self.path.mkdir(mode=0o755)
                self.data = self.path / 'data'
                self.transport = self.path / 'transport'
                self.backups = self.path / 'backups'
                private_directory(self.data, broker, broker)
                private_directory(self.transport, broker, client_gid, 0o2710)
                private_directory(self.backups, broker, broker)
                self.database = self.data / 'history.sqlite'
                self.socket = self.transport / 'agenturm-native-storage-v1.sock'
                self.config = self.path / 'broker.json'
                self.config.write_text(json.dumps({'version': 1, 'socket': str(self.socket),
                    'receiver_id': f'receiver:{label}', 'service_uid': broker, 'service_gid': broker}))
                self.process = None

            def service_command(self):
                return [app.with_name('agenturm-storage-service'), '--data-root', self.data,
                    '--transport-dir', self.transport, '--receiver-id', f'receiver:{self.label}',
                    '--client-uid', str(collector), '--client-gid', str(client_gid),
                    '--native-reader', f'{reader}:{client_gid}', '--native-grant-admin', f'{admin}:{client_gid}',
                    '--native-maintenance', f'{maintainer}:{client_gid}', '--native-sources', SOURCE,
                    '--native-backup-root', self.backups]

            def start(self):
                self.process = subprocess.Popen(self.service_command(), cwd=root,
                    user=broker, group=broker, extra_groups=[])
                wait_for(lambda: self.socket.exists() and self.database.exists() and
                         stat.S_IMODE(self.socket.stat().st_mode) == 0o660, self.process,
                         'Broker preparation failed')

            def cmd(self, *arguments):
                return [app, 'broker', *arguments, '--broker-config', self.config]

            def import_span(self, suffix, ended):
                payload = self.path / 'spans.json'
                payload.write_text(json.dumps({'resourceSpans': [{'scopeSpans': [{'spans': [{
                    'traceId': TRACE, 'spanId': suffix * 8, 'name': DISCARDED,
                    'startTimeUnixNano': str(ended - 1), 'endTimeUnixNano': str(ended),
                    'status': {'code': 1}, 'attributes': [
                        {'key': 'session.id', 'value': {'stringValue': 'eval-session'}}],
                }]}]}]}))
                run(collector, self.cmd('import-traces', '--source-id', SOURCE, '--file', payload))

            def import_detail(self, body):
                payload = self.path / 'results.json'
                payload.write_text(json.dumps({'results': [{'trace_id': TRACE, 'passed': False,
                    'task_id': 'evaluation-fixture', 'error': body, 'details': {'raw': DISCARDED},
                    'scores': [{'scorer': 'matcher', 'score': 0, 'detail': SCORE}]}]}))
                return run(collector, self.cmd('import-evals', '--source-id', SOURCE,
                    '--file', payload, '--include-detail'))

            def metadata(self):
                raw = run(reader, self.cmd('evals', '--source-id', SOURCE, '--trace-id', TRACE, '--json'))
                assert all(text not in raw for text in [BODY_A, BODY_B, SCORE, DISCARDED])
                return json.loads(raw)

            def cursor(self):
                with closing(sqlite3.connect(f'file:{self.database}?mode=ro', uri=True)) as db:
                    return db.execute('SELECT cursor FROM agenturm_source_state WHERE source_id=?', (SOURCE,)).fetchone()

            def grant(self, verb, uid=admin, code=0):
                run(uid, self.cmd('grant', verb, '--binding', binding_file), code=code)

            def preview(self, selection, uid=reader, code=0):
                selection_file.write_text(json.dumps(selection))
                assert BODY_A not in selection_file.read_text() and BODY_B not in selection_file.read_text()
                return run(uid, self.cmd('eval-detail', 'preview', '--selection', selection_file), code=code)

            def restore(self, backup):
                stop(self.process)
                self.data.rename(self.path / 'before-restore')
                private_directory(self.data, broker, broker)
                run(broker, [*self.service_command(), '--native-restore', backup], gid=broker)
                self.start()

        a, b = Receiver('eval-a'), Receiver('eval-b')
        gateway = None
        try:
            a.start()
            a.import_span('22', 200)
            gateway = subprocess.Popen(a.cmd('serve-history', '--listen', '127.0.0.1:0',
                    '--token-file', token_file), cwd=root, stdout=subprocess.PIPE, text=True,
                    user=reader, group=client_gid, extra_groups=[])
            assert select.select([gateway.stdout], [], [], 8)[0]
            endpoint = gateway.stdout.readline().strip().split()[-1]
            _, info = request(endpoint, token, {'op': 'identity'})
            binding = info['history_identity']

            def snapshot():
                code, reply = request(endpoint, token, {'op': 'history', 'query': {'limit': 1},
                    'selection': {'kind': 'observations'}}, binding)
                assert code == 200
                return reply['result']['Ok']['value']['snapshot_id']

            old, cursor = snapshot(), a.cursor()
            assert a.metadata() is None
            assert 'Imported 1' in a.import_detail(BODY_A)
            selected = a.metadata()['detail_selection']
            assert a.cursor() == cursor and selected['snapshot_id'] == old + 1
            a.preview(selected, code=1)
            a.grant('approve', uid=reader, code=1)
            a.grant('approve')
            expected_a = {'provenance': 'imported evaluation text', 'detail': {
                'error': BODY_A, 'scores': [{'score_index': 0, 'detail': SCORE}]}}
            assert json.loads(a.preview(selected)) == expected_a
            a.preview({**selected, 'snapshot_id': old}, code=1)
            a.preview({**selected, 'revision': selected['revision'] + 1}, code=1)
            a.preview(selected, uid=collector, code=1)
            a.grant('revoke')
            a.preview(selected, code=1)
            a.grant('approve')
            assert json.loads(a.preview(selected)) == expected_a
            assert 'Replayed 1' in a.import_detail(BODY_A)
            assert a.cursor() == cursor and snapshot() == selected['snapshot_id']
            terminal_state = root / 'terminal-reader'
            private_directory(terminal_state, reader, client_gid)
            for columns, rows, close in [(60, 18, b'\x1b'), (120, 32, b'\x1b'), (120, 32, b'q'), (60, 18, b'\x03')]:
                cockpit_preview(a, terminal_state, reader, client_gid, admin, binding_file, columns, rows, close)
                a.preview(selected, code=1)
                a.grant('approve')

            operation = {'op': 'selected_evals', 'source_id': SOURCE, 'query': {'trace_id': TRACE,
                'evidence_snapshot': old, 'view': {'view': 'history', 'trace_id': TRACE, 'limit': 1}}}
            code, reply = request(endpoint, token, operation, binding)
            assert code == 200 and reply['result']['Ok']['value']['current_revision'] == 1
            assert all(text not in json.dumps(reply) for text in [BODY_A, BODY_B, SCORE, DISCARDED])
            assert request(endpoint, token, {'op': 'reveal_eval_detail', 'selection': selected}, binding)[0] == 400
            assert request_get(endpoint, token, '/api/traces', binding)[0] == 200
            stop(gateway)

            b.start()
            b.import_span('22', 200)
            assert 'Imported 1' in b.import_detail(BODY_B)
            b.grant('approve')
            other = b.metadata()['detail_selection']
            assert other['snapshot_id'] == selected['snapshot_id']
            assert BODY_B in b.preview(other)
            b.preview(selected, code=1)
            stop(b.process)

            backup = json.loads(run(maintainer, a.cmd('backup')))
            a.restore(backup['name'])
            fresh = a.metadata()['detail_selection']
            a.preview(fresh, code=1)
            a.grant('approve')
            a.preview(selected, code=1)
            assert json.loads(a.preview(fresh)) == expected_a

            a.import_span('33', 900)
            run(maintainer, a.cmd('prune', '--before-unix-ns', '500'))
            a.preview(fresh, code=1)
            assert a.metadata()['revision'] == 1 and not a.metadata().get('detail_selection')
            checkpoint = a.cursor()
            assert 'Replayed 1' in a.import_detail(BODY_A)
            assert a.cursor() == checkpoint and not a.metadata().get('detail_selection')
            print('PASS private eval broker roles, immutable draft selection, isolated current metadata, backup and prune', flush=True)
        finally:
            for process in [gateway, b.process, a.process]:
                if process is not None:
                    stop(process)


if __name__ == '__main__':
    main()
