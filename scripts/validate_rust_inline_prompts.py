"""Synthetic local-private prompt PTY qualification; not paired UI parity.

Uses only an owned fixture and explicit local collection configuration. The
standalone writer lease means CLI revocation is tested after closing the UI;
live broker revocation requires its separate isolated root-capable fixture.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys
import time

p = argparse.ArgumentParser()
p.add_argument('--repo', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
args = p.parse_args()
repo = args.repo.resolve()
sys.path.insert(0, str(repo / 'scripts'))
from validate_rust_ui_parity import AcceptanceCockpit, environment, exact_copy
from validation_support import create_owned_directory, file_hash

root = args.output.resolve()
create_owned_directory(root)
for name in ('home', 'tmp', 'empty-path'):
    (root / name).mkdir(mode=0o700)
binary = root / 'agenturm-under-test'
exact_copy(repo / 'target/debug/agenturm', file_hash(repo / 'target/debug/agenturm'), binary)
adapter = repo / 'target/debug/agenturm-pi-harness'
host = repo / 'target/debug/agenturm-plugin-host'
env = environment(root)
env['AGENTURM_VALIDATION_APP_ROOT'] = str(repo)
source = root / 'synthetic-pi.jsonl'
rows = [{'type':'session', 'id':'uiq-private-session'}]
for index, marker in enumerate(['UIQ_OLDER_REQUEST_MARKER', 'UIQ_MIDDLE_REQUEST_MARKER', 'UIQ_LATEST_REQUEST_MARKER'], 1):
    rows.append({'type':'message', 'id':f'prompt-{index}', 'timestamp':f'2026-11-24T12:00:0{index}Z', 'message':{'role':'user','content':marker+'\n'+'    Recorded fixture line 界 é.\n'*35}})
source.write_text(''.join(json.dumps(row, ensure_ascii=False)+'\n' for row in rows))
catalog = root / 'catalog.json'
catalog.write_text(json.dumps({'version':1,'plugins':[{'plugin_id':'example.rust-pi-harness','adapter':str(adapter),'sha256':file_hash(adapter),'protocol':1,'features':['source-adapter/1','configured-root/1'],'observation_format':'agenturm-observations/1'}]}))
config = root / 'sources.json'
config.write_text(json.dumps({'version':2,'plugin_host':str(host),'plugin_catalog':str(catalog),'sources':[{'plugin_id':'example.rust-pi-harness','source_root':str(source),'source_id':'harness.pi.uiq'}]}))
for path in (source, catalog, config): path.chmod(0o600)
env['AGENTURM_CONTENT_CONFIG'] = str(config)
db = root / 'synthetic.db'
def run(*words):
    result = subprocess.run([str(binary), *map(str,words)], env=env, capture_output=True, timeout=35)
    assert result.returncode == 0, (words[:2],result.stderr.decode(errors='replace'))
    return result.stdout.decode()
run('collect','--config',config,'--db',db)
query = run('query','--db',db)
assert 'UIQ_' not in query, 'captured body must not leak into public query'
metadata = json.loads(query)['observations']
assert len([row for row in metadata if row['observation']['kind']=='message']) == 3
report = {'kind':'local-private-inline-pty-only','source_sha':subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),'binary_sha256':file_hash(binary),'adapter_sha256':file_hash(adapter),'plugin_host_sha256':file_hash(host),'fixture':'uiq-private-native-v1','source_fixture_sha256':file_hash(source),'journeys':[],'limitations':['Not broker/attached receiver qualification','Not an installed release archive','Not paired with identical Python data','Not clipboard/terminal-emulator color QA']}
try:
    for width,height in ((120,32),(80,24),(60,18)):
        run('content','grant','--db',db,'--config',config,'--source-id','harness.pi.uiq')
        journey = {'size':[width,height],'frames':[],'status':'running'}
        report['journeys'].append(journey)
        with (root/f'{width}x{height}.pty').open('xb',buffering=0) as transcript:
            cockpit = AcceptanceCockpit([str(binary),'cockpit','--db',str(db)],env,transcript,width=width,height=height)
            def frame(label, predicate, keys=b'', forbidden=()):
                if keys: cockpit.send(keys)
                text = cockpit.frame(label,predicate)
                assert not any(value in text for value in forbidden), (label,text)
                path=root/f'{width}x{height}-{label}.txt'
                path.write_text(text+'\n')
                journey['frames'].append({'label':label,'keys_hex':keys.hex(),'artifact':path.name,'sha256':file_hash(path)})
                return text
            try:
                frame('roster', lambda t:'Agenturm command center' in t and 'All 1' in t)
                frame('prompt-navigator',lambda t:'3 loaded' in t or 'Request 3 of 3' in t,b'P')
                frame('reader-unnormalized',lambda t:'Readable form unavailable' in t,b'\x1b[C',forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                frame('latest-original',lambda t:'UIQ_LATEST_REQUEST_MARKER' in t,b'z')
                frame('older-read',lambda t:'Readable form unavailable' in t,b'[',forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                frame('older-original',lambda t:'UIQ_MIDDLE_REQUEST_MARKER' in t,b'z')
                frame('reader-cleared',lambda t:'Readable form unavailable' not in t and 'UIQ_MIDDLE_REQUEST_MARKER' not in t,b'\x1b',forbidden=('UIQ_MIDDLE_REQUEST_MARKER','UIQ_LATEST_REQUEST_MARKER'))
                frame('prompt-help',lambda t:'Help' in t,b'\x1bOP')
                frame('help-return',lambda t:'Help' not in t and ('Requests' in t or 'Request' in t),b'\x1b')
                frame('roster-return',lambda t:'Agenturm command center' in t,b'\x1b',forbidden=('UIQ_MIDDLE_REQUEST_MARKER','UIQ_LATEST_REQUEST_MARKER'))
                frame('inspector',lambda t:'Overview' in t and 'Details' in t and 'Agenturm command center' not in t,b'\r')
                frame('inspector-prompts',lambda t:'Request 3 of 3' in t or '3 loaded' in t,b'P')
                frame('inspector-reader',lambda t:'Readable form unavailable' in t,b'\x1b[C')
                frame('inspector-original',lambda t:'UIQ_LATEST_REQUEST_MARKER' in t,b'z')
                deadline=time.monotonic()+28
                while time.monotonic()<deadline:
                    cockpit.read(0.1)
                    text=cockpit.screen.contents()
                    if 'UIQ_LATEST_REQUEST_MARKER' not in text and 'expired' in text.lower(): break
                frame('expiry-redaction',lambda t:'UIQ_LATEST_REQUEST_MARKER' not in t and 'expired' in t.lower(),forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                frame('inspector-return',lambda t:'Overview' in t and 'Details' in t,b'\x1b',forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                cockpit.finish(b'q')
                cockpit.close()
                # Standalone cockpit owns the writer lease. Revoke through the
                # CLI only after it releases that lease; live broker revocation
                # is a separate root-capable fixture, not claimed by this run.
                run('content','revoke','--db',db,'--source-id','harness.pi.uiq')
                cockpit = AcceptanceCockpit([str(binary),'cockpit','--db',str(db)],env,transcript,width=width,height=height)
                frame('reopened-roster',lambda t:'Agenturm command center' in t and 'All 1' in t)
                frame('denied-reopen',lambda t:'Prompt access denied' in t and 'Agenturm command center' not in t,b'P',forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                frame('denied-return',lambda t:'Agenturm command center' in t,b'\x1b',forbidden=('UIQ_LATEST_REQUEST_MARKER',))
                cockpit.finish(b'q')
                journey.update(status='passed',termios_restored=True)
            finally:
                cockpit.close()
    report['status']='passed-local-private-pty-only'
except Exception as error:
    report['status']='failed'
    report['error']=repr(error)
    if report['journeys']:
        report['journeys'][-1]['status']='failed'
        report['journeys'][-1]['last_frame']=cockpit.screen.contents()
finally:
    (root/'results.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'status':report['status'],'error':report.get('error')}))
sys.exit(0 if report['status']=='passed-local-private-pty-only' else 1)
