"""Local, bounded attention receipts and at-most-once notification attempts.

Call this module from background workers. It stores identifiers and generic
incident labels, never task text, commands, prompts, or receiver credentials.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import sqlite3
import time


SCHEMA_VERSION = 2
MAX_INCIDENTS = 10_000
ACK_RETENTION_SECONDS = 90 * 86400
RESOLVED_RETENTION_SECONDS = 30 * 86400


class WorkflowStoreError(ValueError):
    """Monitoring may continue, but durable workflow state is unavailable."""


class WorkflowSchemaError(WorkflowStoreError):
    pass


def default_path():
    return Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local' / 'state')) / 'agenturm' / 'workflow.sqlite3'


@dataclass(frozen=True)
class ObservationResult:
    records: tuple[dict, ...]
    unpersisted: tuple[str, ...] = ()

    @property
    def warning(self):
        return (f'Workflow storage capacity reached; {len(self.unpersisted)} incident(s) are memory only. '
                'Acknowledgement and desktop delivery are unavailable for them.' if self.unpersisted else '')


def _text(value, field, limit=512):
    if not isinstance(value, str) or not value or len(value) > limit or any(ord(c) < 32 for c in value):
        raise WorkflowStoreError('Invalid workflow ' + field)
    return value


def _record(value):
    if hasattr(value, 'to_record'):
        value = value.to_record()
    if not isinstance(value, dict):
        raise WorkflowStoreError('Invalid workflow incident')
    # An allowlist prevents future UI fields from silently persisting content.
    record = {key: value.get(key) for key in (
        'id', 'namespace', 'scope_key', 'kind', 'anchor', 'source_time_ns',
        'first_observed_time', 'priority', 'reason', 'persistent', 'alertable', 'execution')}
    for name in ('id', 'namespace', 'scope_key', 'kind', 'anchor'):
        record[name] = _text(record[name], name)
    record['source_time_ns'] = max(0, int(record.get('source_time_ns') or 0))
    if record['source_time_ns'] > 2 ** 63 - 1:
        raise WorkflowStoreError('Invalid workflow source timestamp')
    record['first_observed_time'] = float(record.get('first_observed_time') or time.time())
    if not math.isfinite(record['first_observed_time']):
        raise WorkflowStoreError('Invalid workflow observation timestamp')
    record['priority'] = int(record.get('priority') or 0)
    record['reason'] = str(record.get('reason') or '')[:300]
    record['persistent'] = bool(record.get('persistent'))
    record['alertable'] = bool(record.get('alertable'))
    source = record.get('execution') or {}
    if not isinstance(source,dict):
        raise WorkflowStoreError('Invalid workflow execution reference')
    execution = {}
    for key in ('producer','agent_id','session_id','run_id','turn_id','kind','source_id','resolution_policy','native_request_key'):
        text = source.get(key) or ''
        execution[key] = _text(text,key) if text else ''
    start = source.get('run_start_ns')
    if start is not None and (not isinstance(start,int) or isinstance(start,bool) or not 0<start<2**63):
        raise WorkflowStoreError('Invalid workflow lifecycle reference')
    end=source.get('run_end_ns')
    if end is not None and (type(end) is not int or end<=0):raise WorkflowStoreError('Invalid workflow execution end')
    execution.update(run_start_ns=start,run_end_ns=end,endpoint_fallback=bool(source.get('endpoint_fallback')))
    record['execution'] = execution
    return record


class WorkflowStore:
    def __init__(self, path=None, *, max_incidents=MAX_INCIDENTS):
        self.path = Path(path).expanduser() if path is not None else default_path()
        self.max_incidents = max(1, min(MAX_INCIDENTS, int(max_incidents)))
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            if self.path.is_symlink():
                raise WorkflowStoreError('Workflow state path must not be a symbolic link')
            try:
                fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            except FileExistsError:
                pass
            else:
                os.close(fd)
            os.chmod(self.path, 0o600)
            with self._transaction() as db:
                db.execute('CREATE TABLE IF NOT EXISTS workflow_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)')
                version = db.execute("SELECT value FROM workflow_meta WHERE key='schema_version'").fetchone()
                if version is not None and version[0]!=str(SCHEMA_VERSION) and not (SCHEMA_VERSION==2 and version[0]=='1'):
                    raise WorkflowSchemaError('Unsupported workflow store schema version ' + str(version[0]))
                db.execute('''CREATE TABLE IF NOT EXISTS workflow_incidents_v1 (
                    namespace TEXT NOT NULL, id TEXT NOT NULL, scope_key TEXT NOT NULL,
                    kind TEXT NOT NULL, anchor TEXT NOT NULL, source_time_ns INTEGER NOT NULL,
                    first_observed REAL NOT NULL, last_observed REAL NOT NULL,
                    resolved_at REAL, payload TEXT NOT NULL, PRIMARY KEY(namespace,id))''')
                db.execute('''CREATE TABLE IF NOT EXISTS workflow_acknowledgements_v1 (
                    namespace TEXT NOT NULL, id TEXT NOT NULL, scope_key TEXT NOT NULL,
                    action TEXT NOT NULL, created_at REAL NOT NULL, PRIMARY KEY(namespace,id))''')
                db.execute('''CREATE TABLE IF NOT EXISTS workflow_notification_claims_v1 (
                    namespace TEXT NOT NULL, id TEXT NOT NULL, scope_key TEXT NOT NULL,
                    claimed_at REAL NOT NULL, updated_at REAL NOT NULL, attempts INTEGER NOT NULL,
                    status TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '', PRIMARY KEY(namespace,id))''')
                db.execute('CREATE INDEX IF NOT EXISTS workflow_incidents_resolved ON workflow_incidents_v1(resolved_at)')
                db.execute('''CREATE TABLE IF NOT EXISTS workflow_error_seen_v1 (
                    id TEXT PRIMARY KEY, seen_at REAL NOT NULL)''')
                db.execute("""CREATE TABLE IF NOT EXISTS workflow_attention_baselines_v1 (
                    namespace TEXT PRIMARY KEY, baseline_time_ns INTEGER NOT NULL,
                    created_at REAL NOT NULL)""")
                db.execute("""CREATE TABLE IF NOT EXISTS workflow_attention_eligibility_v1 (
                    namespace TEXT NOT NULL,id TEXT NOT NULL,scope_key TEXT NOT NULL,
                    eligible INTEGER NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(namespace,id))""")
                db.execute("""CREATE TABLE IF NOT EXISTS workflow_native_senders_v1 (
                    sender_id TEXT PRIMARY KEY,namespace TEXT NOT NULL,binding_hash TEXT NOT NULL,
                    sequence INTEGER NOT NULL DEFAULT 0,rejected INTEGER NOT NULL DEFAULT 0,last_reason TEXT NOT NULL DEFAULT '')""")
                db.execute("""CREATE TABLE IF NOT EXISTS workflow_native_events_v1 (
                    event_key TEXT PRIMARY KEY,sender_id TEXT NOT NULL,sequence INTEGER NOT NULL,event_hash TEXT NOT NULL,
                    payload TEXT NOT NULL,sent INTEGER NOT NULL DEFAULT 0,UNIQUE(sender_id,sequence))""")
                db.execute("""CREATE TABLE IF NOT EXISTS workflow_native_requests_v1 (
                    sender_id TEXT NOT NULL,request_id TEXT NOT NULL,kind TEXT NOT NULL,resolved INTEGER NOT NULL,
                    event_key TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(sender_id,request_id))""")
                db.execute('CREATE INDEX IF NOT EXISTS workflow_native_namespace_v1 ON workflow_native_senders_v1(namespace,sender_id)')
                db.execute("INSERT INTO workflow_meta VALUES('schema_version',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (str(SCHEMA_VERSION),))
        except (OSError, sqlite3.Error) as exc:
            raise WorkflowStoreError('Workflow storage unavailable: ' + str(exc)) from exc

    @contextmanager
    def _transaction(self, *, write=True):
        db = None
        try:
            db = sqlite3.connect(self.path, timeout=.5)
            db.row_factory = sqlite3.Row
            if write:
                db.execute('BEGIN IMMEDIATE')
            yield db
            if write:
                db.commit()
        except sqlite3.Error as exc:
            if db is not None:
                db.rollback()
            raise WorkflowStoreError('Workflow storage unavailable: ' + str(exc)) from exc
        except BaseException:
            if db is not None:
                db.rollback()
            raise
        finally:
            if db is not None:
                db.close()

    @staticmethod
    def _cleanup(db, now):
        acks = db.execute('DELETE FROM workflow_acknowledgements_v1 WHERE created_at<?',
                          (now - ACK_RETENTION_SECONDS,)).rowcount
        cutoff = now - RESOLVED_RETENTION_SECONDS
        db.execute('''DELETE FROM workflow_notification_claims_v1 AS claims WHERE EXISTS (
            SELECT 1 FROM workflow_incidents_v1 AS incidents
            WHERE incidents.namespace=claims.namespace AND incidents.id=claims.id
            AND incidents.resolved_at IS NOT NULL AND incidents.resolved_at<?)''', (cutoff,))
        # A receipt keeps its minimal execution reference available for its full
        # 90-day lifetime. These rows contain no task/prompt/command content.
        incidents = db.execute('''DELETE FROM workflow_incidents_v1 AS incidents
            WHERE resolved_at IS NOT NULL AND resolved_at<? AND NOT EXISTS (
              SELECT 1 FROM workflow_acknowledgements_v1 AS ack
              WHERE ack.namespace=incidents.namespace AND ack.id=incidents.id
                AND ack.scope_key=incidents.scope_key)''', (cutoff,)).rowcount
        db.execute('''DELETE FROM workflow_attention_eligibility_v1 AS eligibility WHERE NOT EXISTS (
            SELECT 1 FROM workflow_incidents_v1 AS incident WHERE incident.namespace=eligibility.namespace AND incident.id=eligibility.id)''')
        return {'acknowledgements': acks, 'resolved_incidents': incidents}

    def cleanup(self, *, now=None):
        with self._transaction() as db:
            return self._cleanup(db, time.time() if now is None else float(now))

    def load_error_seen(self):
        with self._transaction(write=False) as db:
            rows = db.execute('SELECT id FROM workflow_error_seen_v1 ORDER BY seen_at DESC,id DESC LIMIT ?',
                              (MAX_INCIDENTS,)).fetchall()
            return tuple(row['id'] for row in reversed(rows))

    def mark_error_seen(self, identities, *, now=None):
        identities = list(dict.fromkeys(identities))
        if len(identities) > MAX_INCIDENTS or any(
                not isinstance(value, str) or len(value) != 64 or
                any(char not in '0123456789abcdef' for char in value) for value in identities):
            raise WorkflowStoreError('Invalid error receipt identity')
        with self._transaction() as db:
            # Idempotent, unlike the explicit reversible workflow acknowledgement.
            db.executemany('INSERT OR IGNORE INTO workflow_error_seen_v1 VALUES(?,?)',
                           [(value, time.time() if now is None else float(now)) for value in identities])
            db.execute('''DELETE FROM workflow_error_seen_v1 WHERE id NOT IN (
                SELECT id FROM workflow_error_seen_v1 ORDER BY seen_at DESC,id DESC LIMIT ?)''', (MAX_INCIDENTS,))

    @staticmethod
    def _rows(db, namespace=None):
        where = ' WHERE incidents.namespace=?' if namespace is not None else ''
        rows = db.execute('''SELECT incidents.*,ack.action AS acknowledgement,
            ack.created_at AS acknowledged_at,claim.status AS delivery_status,
            claim.detail AS delivery_detail,claim.attempts AS delivery_attempts,
            eligibility.eligible AS attention_eligible,eligibility.reason AS attention_eligibility_reason
            FROM workflow_incidents_v1 AS incidents
            LEFT JOIN workflow_acknowledgements_v1 AS ack
              ON ack.namespace=incidents.namespace AND ack.id=incidents.id AND ack.scope_key=incidents.scope_key
            LEFT JOIN workflow_notification_claims_v1 AS claim
              ON claim.namespace=incidents.namespace AND claim.id=incidents.id AND claim.scope_key=incidents.scope_key
            LEFT JOIN workflow_attention_eligibility_v1 AS eligibility
              ON eligibility.namespace=incidents.namespace AND eligibility.id=incidents.id AND eligibility.scope_key=incidents.scope_key
            ''' + where + ' ORDER BY incidents.first_observed,incidents.id LIMIT ?',
            ([namespace] if namespace is not None else []) + [MAX_INCIDENTS]).fetchall()
        result = []
        for row in rows:
            record = json.loads(row['payload'])
            record.update(first_observed_time=row['first_observed'], last_observed_time=row['last_observed'],
                          resolved_at=row['resolved_at'], acknowledgement=row['acknowledgement'],
                          acknowledged_at=row['acknowledged_at'], delivery_status=row['delivery_status'],
                          delivery_detail=row['delivery_detail'], delivery_attempts=row['delivery_attempts'] or 0,
                          attention_eligible=bool(row['attention_eligible']),
                          attention_eligibility_reason=row['attention_eligibility_reason'] or '')
            result.append(record)
        return tuple(result)

    def load(self, namespace=None, *, now=None):
        with self._transaction() as db:
            self._cleanup(db, time.time() if now is None else float(now))
            return self._rows(db, namespace)

    def observe(self, incidents, *, resolved=(), now=None, inventory=None, new_ids=(), comparable_ids=()):
        now = time.time() if now is None else float(now)
        records = [_record(incident) for incident in incidents]
        if len(records) > MAX_INCIDENTS:
            raise WorkflowStoreError('Workflow observation batch exceeds the 10,000 incident limit')
        omitted = []
        with self._transaction() as db:
            self._cleanup(db, now)
            # Resolution is explicit; a missing source or failed capture never
            # implicitly resolves its previously observed incidents.
            for namespace, incident_id in list(resolved)[:MAX_INCIDENTS]:
                db.execute('UPDATE workflow_incidents_v1 SET resolved_at=COALESCE(resolved_at,?) WHERE namespace=? AND id=?',
                           (now, namespace, incident_id))
            count = db.execute('SELECT COUNT(*) FROM workflow_incidents_v1').fetchone()[0]
            for record in records:
                if not record['persistent']:
                    omitted.append(record['id'])
                    continue
                exists = db.execute('SELECT scope_key FROM workflow_incidents_v1 WHERE namespace=? AND id=?',
                                    (record['namespace'], record['id'])).fetchone()
                if exists is None and count >= self.max_incidents:
                    omitted.append(record['id'])
                    continue
                if exists is not None and exists[0] != record['scope_key']:
                    raise WorkflowStoreError('Workflow incident identity conflicts with its stored scope')
                db.execute('''INSERT INTO workflow_incidents_v1 VALUES(?,?,?,?,?,?,?,?,NULL,?)
                    ON CONFLICT(namespace,id) DO UPDATE SET
                    source_time_ns=excluded.source_time_ns,last_observed=excluded.last_observed,
                    payload=excluded.payload''',
                    (record['namespace'], record['id'], record['scope_key'], record['kind'], record['anchor'],
                     record['source_time_ns'], record['first_observed_time'], now,
                     json.dumps(record, ensure_ascii=False, separators=(',', ':'))))
                if exists is None:
                    count += 1
                if record['kind'] not in {'approval','reply'}:
                    db.execute('''UPDATE workflow_incidents_v1 SET resolved_at=COALESCE(resolved_at,
                        (SELECT created_at FROM workflow_acknowledgements_v1 AS ack
                         WHERE ack.namespace=? AND ack.id=? AND ack.scope_key=?)) WHERE namespace=? AND id=?''',
                        (record['namespace'],record['id'],record['scope_key'],record['namespace'],record['id']))
            self._attention_observation(db,records,inventory or {},set(new_ids),set(comparable_ids),now)
            saved = self._rows(db)
        return ObservationResult(saved, tuple(omitted))

    @staticmethod
    def _attention_observation(db,records,inventory,new_ids,comparable_ids,now):
        """Baseline and first delivery eligibility commit with observations.

        A source clock must be explicitly comparable to the receiver clock.
        Unknown timestamps require an observed request transition in a scope
        that was already fresh, rather than a reconstructed offline age.
        """
        for namespace,stamp in inventory.items():
            _text(namespace,'namespace')
            if type(stamp) is not int or not 0<stamp<2**63:continue
            baseline=db.execute('SELECT baseline_time_ns FROM workflow_attention_baselines_v1 WHERE namespace=?',(namespace,)).fetchone()
            if baseline is None:
                if db.execute('SELECT COUNT(*) FROM workflow_attention_baselines_v1').fetchone()[0]>=MAX_INCIDENTS:
                    db.execute('''DELETE FROM workflow_attention_baselines_v1 WHERE namespace IN (
                        SELECT baseline.namespace FROM workflow_attention_baselines_v1 baseline
                        WHERE NOT EXISTS (SELECT 1 FROM workflow_incidents_v1 incident WHERE incident.namespace=baseline.namespace)
                        ORDER BY created_at LIMIT 1)''')
                    if db.execute('SELECT COUNT(*) FROM workflow_attention_baselines_v1').fetchone()[0]>=MAX_INCIDENTS:
                        raise WorkflowStoreError('Workflow receiver baseline capacity reached')
                db.execute('INSERT INTO workflow_attention_baselines_v1 VALUES(?,?,?)',(namespace,stamp,now))
                db.execute("""INSERT INTO workflow_attention_eligibility_v1
                    SELECT namespace,id,scope_key,0,'Initial receiver inventory'
                    FROM workflow_incidents_v1 WHERE namespace=? ON CONFLICT DO NOTHING""",(namespace,))
                continue
            for record in records:
                if record['namespace']!=namespace:continue
                source_stamp=record['source_time_ns']
                comparable=record['id'] in comparable_ids and source_stamp>0
                if comparable and source_stamp<=baseline[0]:
                    eligible,reason=False,'Request predates initial receiver inventory'
                elif comparable:
                    eligible,reason=True,'Recorded source time is newer than receiver baseline'
                elif record['id'] in new_ids:
                    eligible,reason=True,'New request observed after a prior fresh scope observation'
                else:
                    eligible,reason=False,'Source time or clock is unavailable; offline chronology is unknown'
                db.execute("""INSERT INTO workflow_attention_eligibility_v1 VALUES(?,?,?,?,?)
                    ON CONFLICT DO NOTHING""",(namespace,record['id'],record['scope_key'],int(eligible),reason))

    def attention_baselines(self):
        with self._transaction(write=False) as db:
            return {row['namespace']:row['baseline_time_ns'] for row in db.execute('SELECT * FROM workflow_attention_baselines_v1')}

    def native_observations(self,namespace):
        with self._transaction(write=False) as db:
            rows=db.execute("""SELECT request.payload,request.resolved,request.kind FROM workflow_native_requests_v1 request
                JOIN workflow_native_senders_v1 sender USING(sender_id)
                JOIN workflow_native_events_v1 event ON event.event_key=request.event_key AND event.sent=1
                WHERE sender.namespace=? ORDER BY request.sender_id,request.request_id LIMIT 1000""",(namespace,)).fetchall()
            return [{**json.loads(row['payload']),'resolved':bool(row['resolved']),'request_kind':row['kind']} for row in rows]

    def native_status(self,namespace=None):
        with self._transaction(write=False) as db:
            where=' WHERE namespace=?' if namespace else ''
            rows=db.execute('SELECT sender_id,namespace,sequence,rejected,last_reason FROM workflow_native_senders_v1'+where+
                ' ORDER BY sender_id LIMIT 1000',([namespace] if namespace else [])).fetchall()
            return [dict(row) for row in rows]

    def acknowledge(self, namespace, incident_id, scope_key, *, action='seen', enabled=True, now=None):
        if action not in {'seen', 'reviewed', 'dismissed'}:
            raise WorkflowStoreError('Invalid acknowledgement action')
        with self._transaction() as db:
            found = db.execute('SELECT 1 FROM workflow_incidents_v1 WHERE namespace=? AND id=? AND scope_key=?',
                               (namespace, incident_id, scope_key)).fetchone()
            if found is None:
                raise WorkflowStoreError('This incident is not persisted; durable acknowledgement is unavailable')
            if enabled:
                db.execute('''INSERT INTO workflow_acknowledgements_v1 VALUES(?,?,?,?,?)
                    ON CONFLICT(namespace,id) DO UPDATE SET scope_key=excluded.scope_key,
                    action=excluded.action,created_at=excluded.created_at''',
                    (namespace, incident_id, scope_key, action, time.time() if now is None else float(now)))
            else:
                db.execute('DELETE FROM workflow_acknowledgements_v1 WHERE namespace=? AND id=? AND scope_key=?',
                           (namespace, incident_id, scope_key))
            db.execute('''UPDATE workflow_incidents_v1 SET resolved_at=?
                WHERE namespace=? AND id=? AND scope_key=? AND kind NOT IN ('approval','reply')''',
                ((time.time() if now is None else float(now)) if enabled else None,namespace,incident_id,scope_key))

    def review_completions(self, references, *, now=None):
        """Atomically review a frozen set; approvals and future arrivals are excluded."""
        references=tuple(dict.fromkeys(references))
        if len(references)>MAX_INCIDENTS:raise WorkflowStoreError('Completion selection exceeds the incident limit')
        stamp=time.time() if now is None else float(now)
        with self._transaction() as db:
            for namespace,identity,scope in references:
                row=db.execute('SELECT kind FROM workflow_incidents_v1 WHERE namespace=? AND id=? AND scope_key=?',(namespace,identity,scope)).fetchone()
                if row is None or row[0]!='completion':raise WorkflowStoreError('Only persisted completion incidents can be reviewed together')
            db.executemany("INSERT INTO workflow_acknowledgements_v1 VALUES(?,?,?,'reviewed',?) ON CONFLICT(namespace,id) DO UPDATE SET action='reviewed',created_at=excluded.created_at,scope_key=excluded.scope_key",
                [(namespace,identity,scope,stamp) for namespace,identity,scope in references])
            db.executemany('UPDATE workflow_incidents_v1 SET resolved_at=? WHERE namespace=? AND id=? AND scope_key=?',
                [(stamp,namespace,identity,scope) for namespace,identity,scope in references])

    def claim(self, namespace, incident_id, scope_key, *, retry=False, now=None):
        now = time.time() if now is None else float(now)
        with self._transaction() as db:
            row = db.execute('''SELECT payload FROM workflow_incidents_v1 WHERE namespace=? AND id=?
                AND scope_key=? AND resolved_at IS NULL''', (namespace, incident_id, scope_key)).fetchone()
            if row is None or not json.loads(row[0]).get('alertable'):
                return False
            if db.execute('SELECT 1 FROM workflow_acknowledgements_v1 WHERE namespace=? AND id=?',
                          (namespace, incident_id)).fetchone():
                return False
            if retry:
                changed = db.execute('''UPDATE workflow_notification_claims_v1 SET status='claimed',
                    claimed_at=?,updated_at=?,attempts=attempts+1,detail=''
                    WHERE namespace=? AND id=? AND scope_key=? AND status IN ('failed','unavailable')''',
                    (now, now, namespace, incident_id, scope_key)).rowcount
                return bool(changed)
            return bool(db.execute('''INSERT OR IGNORE INTO workflow_notification_claims_v1
                VALUES(?,?,?,?,?,1,'claimed','')''', (namespace, incident_id, scope_key, now, now)).rowcount)

    def finish_delivery(self, namespace, incident_id, *, status, detail='', now=None):
        if status not in {'delivered', 'failed', 'unavailable'}:
            raise WorkflowStoreError('Invalid notification delivery status')
        with self._transaction() as db:
            db.execute('''UPDATE workflow_notification_claims_v1 SET status=?,detail=?,updated_at=?
                WHERE namespace=? AND id=? AND status='claimed' ''',
                (status, str(detail)[:300], time.time() if now is None else float(now), namespace, incident_id))
