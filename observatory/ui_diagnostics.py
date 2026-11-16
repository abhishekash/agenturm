"""Read-only local collection diagnostics and retention guidance."""
import math
from rich.text import Text
from textual import on
from textual.app import ComposeResult
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, RichLog

from .terminal import clean

RETENTION = '''RETENTION · logical deletion, not secure erasure
On the receiver host, choose your database and retention period explicitly:
  agenturm --db /path/to/observatory.db prune --older-than-days 7
For ongoing cleanup, start the receiver with serve --retention-days 7.
No retention policy is assumed here; the receiver does not report its policy.
Deleted content can remain in SQLite free pages and backups. Native source
logs are separate and are not deleted by pruning telemetry.'''

FIRST_RUN = '''FIND YOUR FIRST SESSION
Start or continue Codex, Claude Code, OpenCode, or Pi in its own terminal, then
return here. Discovery normally looks back 24 hours; Reviews are hidden.
If no session appears, check Diagnostics for read errors and source coverage.
For SDK/OTLP agents, point the producer at your local receiver using the
integration guide in docs/receiver.md. Agenturm observes; it does not run agents.'''


def first_run_guidance(local, *, read_errors=None, sessions_error=''):
    """Explain an empty attention view from receiver evidence, not assumptions."""
    local = local or {}
    sessions = local.get('sessions') or []
    errors = read_errors or {}
    diagnostics = [row.get('diagnostics') or {} for row in sessions]
    if isinstance(local.get('opencode_diagnostics'), dict):
        diagnostics.append(local['opencode_diagnostics'])
    delayed = bool(errors or sessions_error or local.get('read_errors'))
    delayed = delayed or any(record.get('backlog_bytes') for record in diagnostics)
    limited = local.get('capture_content') is False or any(
        row.get('capture_content') is False for row in sessions)

    if not sessions and local.get('enabled') is False:
        return ('Source discovery is disabled. Enable local source discovery, then check Diagnostics (d).',)
    if not sessions:
        if delayed:
            return ('No recent supported source is available yet; collection is delayed, so request state may be incomplete.',
                    'Check Diagnostics (d), then start or continue a supported agent in its native terminal.')
        return ('No recent supported source discovered; this is not a complete zero-request result.',
                'Start or continue a supported agent in its native terminal, then check Diagnostics (d).')
    if delayed:
        return ('Collection is delayed; the request view may be incomplete.',
                'Check the collection error or backlog in Diagnostics (d) before treating this as empty.')
    if limited:
        return ('No recorded request is available in this coverage; capture is metadata-only for at least one source.',
                'Check the source capture policy in Diagnostics (d).')
    return ('No recorded approval/reply requests need response in the available coverage.',
            'Continue work; new eligible requests appear here. Press d for collection diagnostics.')


def storage_status_lines(record):
    """Do not infer a failed write from an older receiver's buffered count."""
    total = record.get('pending_storage_records')
    valid_total = type(total) is int and total >= 0
    amount = str(total) if valid_total else 'not reported'
    state = record.get('storage_state')
    known = type(record.get('storage_diagnostics_version')) is int and record['storage_diagnostics_version'] == 1
    retry = record.get('storage_retry_pending')
    running = record.get('storage_attempt_in_flight')
    if not known:
        return ['Storage state: not reported · buffered batch: ' + amount + ' records']
    if state == 'retry' and retry is True:
        label = 'Retry needed'
    elif state == 'writing' and retry is False and running is True and valid_total:
        label = 'Writing native batch'
    elif state == 'idle' and retry is False and running is False and valid_total and total == 0:
        return ['Storage: Up to date · no buffered batch']
    else:
        label = 'State unknown'
    lines = ['Storage: ' + label + ' · buffered batch: ' + amount + ' records']
    stages = {'prepared': 'Preparing delivery', 'capture_policy': 'Checking capture policy',
        'redaction': 'Applying capture policy', 'spans': 'Writing spans',
        'logs': 'Writing activity', 'collector_metadata': 'Updating collector metadata'}
    def age(key):
        value = record.get(key)
        return f'{value:.1f}s' if type(value) in (int, float) and math.isfinite(value) and value >= 0 else 'not reported'
    lines.append('Stage: ' + stages.get(record.get('storage_stage'), 'not reported') +
        ' · batch age: ' + age('storage_batch_age_seconds') +
        ' · last progress: ' + age('storage_progress_age_seconds') + ' ago')
    if state == 'retry' and retry is True:
        lines.append('Agenturm retries this batch before reading more source records.')
    return lines


def diagnostics_text(local,*,native_bootstrap=False):
    if native_bootstrap:
        lines=['LOCAL COLLECTION DIAGNOSTICS · PARTIAL','History indexing · native observations only.',
            'Historical read-error, source and hierarchy totals: unavailable.',
            f"Observed native rows: {len(local.get('sessions')or [])}"]
        for row in local.get('sessions')or []:
            diagnostics=row.get('diagnostics')or {}
            lines.extend(['',str(row.get('runtime')or 'Native')+' · '+str(row.get('agent_id')or '')[-8:],
                'Last successful collector read (Unix ns): '+str(row.get('collector_updated_ns')or 'not reported'),
                'Backlog bytes: '+str(diagnostics.get('backlog_bytes','not reported')),
                *storage_status_lines(diagnostics)])
        return '\n'.join(lines)
    lines = ['LOCAL COLLECTION DIAGNOSTICS',
             'Local paths may identify projects/users. No endpoint credentials or raw events are shown.',
             f"Discovery: {'enabled' if local.get('enabled') else 'disabled or not reported'} · lookback {local.get('lookback_hours', 'not reported')}h",
             f"Read errors: {local.get('read_errors', 0)} · OpenCode read errors: {local.get('opencode_read_errors', 0)}",
             f"Omitted older sources: {local.get('omitted_sources', 0)} · OpenCode: {local.get('opencode_omitted_sessions', 0)}"]
    hierarchy = local.get('hierarchy') or {}
    error=local.get('last_read_error') or {}
    if error:
        lines.append(f"Last collection error: {error.get('operation','Unknown stage')} · {error.get('kind','Error')} · {error.get('message','No detail reported')} · Unix ns {error.get('at_ns','not reported')}")
    if hierarchy:
        lines.append(
            f"Hierarchy: {hierarchy.get('nodes', 0)} nodes · {hierarchy.get('announced', 0)} spawn announcements · "
            f"coverage {hierarchy.get('coverage', 'unknown')} · "
            f"{hierarchy.get('write_errors', 0)} persistence errors · revision {hierarchy.get('revision', 'not reported')}")
    diagnostics = [row.get('diagnostics') or {} for row in local.get('sessions', [])]
    if isinstance(local.get('opencode_diagnostics'), dict):
        diagnostics.append(local['opencode_diagnostics'])
    seen = set()
    for record in diagnostics:
        path = record.get('source_path')
        if not path or path in seen:
            continue
        seen.add(path)
        lines.extend(['', str(path),
                      f"Last successful read (Unix ns): {record.get('last_success_ns') or 'not reported'}",
                      f"Last source write (Unix ns): {record.get('source_mtime_ns') or 'not reported'} · bytes read / size: {record.get('bytes_read', 'not reported')} / {record.get('source_size_bytes', 'not reported')}",
                      f"Last parsed batch: {record.get('records_last_poll', 'not reported')} records · {record.get('spans_last_poll', 'not reported')} generated spans · {record.get('logs_last_poll', 'not reported')} generated activity events",
                      f"Parsed since monitor start: {record.get('bytes_read_total', 'not reported')} bytes · {record.get('records_total', 'not reported')} records · {record.get('spans_total', 'not reported')} generated spans · {record.get('logs_total', 'not reported')} generated activity events",
                      f"OpenCode sessions: {record.get('sessions_last_poll', 'not reported')} · latest session update (Unix ns): {record.get('latest_session_update_ns', 'not reported')} · payload bytes last poll: {record.get('payload_bytes_last_poll', 'not reported')}",
                      f"Backlog bytes: {record.get('backlog_bytes', 'not reported')} · skipped oversized: {record.get('skipped_oversized_records', 'not reported')}",
                      *storage_status_lines(record),
                      f"Schema errors: {record.get('schema_errors', 'not reported')} · skipped records: {record.get('skipped_records', 'not reported')}"])
    if not seen:
        lines.extend(['', 'Per-source read diagnostics not reported by this receiver.'])
    lines.extend(['', RETENTION, '', FIRST_RUN])
    # Literal text, not terminal escape sequences or Rich markup from source paths.
    return '\n'.join(clean(line) for line in '\n'.join(lines).splitlines())


class DiagnosticsScreen(ModalScreen):
    DEFAULT_CSS = '''
    DiagnosticsScreen { align: center middle; background: $background 70%; }
    #diagnostics-dialog { width: 90%; height: 90%; border: round $primary; background: $surface; padding: 1 2; }
    #diagnostics-log { height: 1fr; }
    #diagnostics-close { width: 100%; }
    '''
    BINDINGS = [('escape', 'dismiss', 'Close')]

    def __init__(self, local, *, native_bootstrap=False):
        super().__init__()
        self.local = local
        self.native_bootstrap=native_bootstrap

    def compose(self) -> ComposeResult:
        with Vertical(id='diagnostics-dialog'):
            yield RichLog(id='diagnostics-log', wrap=True, markup=False)
            yield Button('Close · Esc', id='diagnostics-close')

    def on_mount(self):
        self.query_one(RichLog).write(Text(diagnostics_text(self.local,native_bootstrap=self.native_bootstrap)))

    @on(Button.Pressed, '#diagnostics-close')
    def close_dialog(self):
        self.dismiss()
