"""Metadata-only native request inventory, maintained with authoritative Activity."""
import json


def initialize(db):
    db.execute('''CREATE TABLE IF NOT EXISTS native_lifecycle_index_v1(
        event_key TEXT PRIMARY KEY,kind TEXT NOT NULL,owner_key TEXT NOT NULL,
        receiver_id TEXT NOT NULL,sender_id TEXT NOT NULL,request_id TEXT NOT NULL,
        sequence INTEGER NOT NULL,scope_key TEXT NOT NULL,state_hash TEXT NOT NULL,payload TEXT NOT NULL)''')
    db.execute('CREATE INDEX IF NOT EXISTS native_request_v1 ON native_lifecycle_index_v1(receiver_id,sender_id,request_id,sequence DESC)')
    db.execute('CREATE INDEX IF NOT EXISTS native_owner_v1 ON native_lifecycle_index_v1(kind,owner_key)')
    db.execute('CREATE TABLE IF NOT EXISTS native_lifecycle_dirty_v1(kind TEXT,owner_key TEXT,PRIMARY KEY(kind,owner_key))')
    for operation in ('INSERT','UPDATE','DELETE'):
        alias='OLD' if operation=='DELETE' else 'NEW'
        old=("INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT OLD.kind,OLD.owner_key WHERE json_type(OLD.payload,'$.native_lifecycle')='object';"
             if operation=='UPDATE' else '')
        deletion=('DELETE FROM native_lifecycle_index_v1 WHERE event_key=OLD.event_key;' if operation=='DELETE' else '')
        db.execute(f'''CREATE TRIGGER IF NOT EXISTS native_activity_{operation.lower()}_v1 AFTER {operation} ON activity_index_v1 BEGIN
            INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT {alias}.kind,{alias}.owner_key
                WHERE json_type({alias}.payload,'$.native_lifecycle')='object';{old}{deletion} END''')
    if not db.execute("SELECT 1 FROM schema_meta WHERE key='native_lifecycle_inventory_v1'").fetchone():
        db.execute("INSERT OR IGNORE INTO activity_dirty_v1 SELECT 'log',digest FROM logs WHERE scope_name='observatory.native-lifecycle.v1'")
        db.execute("INSERT INTO schema_meta VALUES('native_lifecycle_inventory_v1','1')")


def replace_owner(db,kind,owner_key,rows):
    from .native_lifecycle import activity_observations,fingerprint
    db.execute('DELETE FROM native_lifecycle_index_v1 WHERE kind=? AND owner_key=?',(kind,owner_key))
    receiver=db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()[0]
    for event_key,row in rows:
        if 'native_lifecycle' not in row:continue
        observations=activity_observations([{**row,'receiver_id':receiver}],receiver)
        for state in observations:
            scope=tuple(state.get(key) or '' for key in ('service','agent_id','session_id','trace_id','turn_id','source_id'))
            signature=fingerprint({key:value for key,value in state.items() if key!='event_source_id'})
            db.execute('INSERT OR REPLACE INTO native_lifecycle_index_v1 VALUES(?,?,?,?,?,?,?,?,?,?)',
                (event_key,kind,owner_key,receiver,state['sender_id'],state['request_id'],state['sequence'],
                 json.dumps(scope,separators=(',',':')),signature,json.dumps(state,separators=(',',':'))))
    db.execute('DELETE FROM native_lifecycle_dirty_v1 WHERE kind=? AND owner_key=?',(kind,owner_key))


def read_inventory(db,receiver):
    # Scalar grouping is confined to optional native metadata. No raw records,
    # prompt bodies or general historical Activity JSON are expanded here.
    conflicted=db.execute('''SELECT 1 FROM native_lifecycle_index_v1 WHERE receiver_id=? GROUP BY sender_id
        HAVING count(DISTINCT scope_key)>1 UNION ALL
        SELECT 1 FROM native_lifecycle_index_v1 WHERE receiver_id=? GROUP BY sender_id,sequence
        HAVING count(DISTINCT state_hash)>1 LIMIT 1''',(receiver,receiver)).fetchone()
    if conflicted:
        return dict(receiver_id=receiver,observations=[],complete=False,warning='Native sender identity or ordering conflicts; no native state was inferred.')
    rows=db.execute('''WITH latest AS (
        SELECT sender_id,request_id,MAX(sequence) AS sequence FROM native_lifecycle_index_v1
        WHERE receiver_id=? GROUP BY sender_id,request_id), selected AS (
        SELECT MIN(event_key) AS event_key FROM native_lifecycle_index_v1 n JOIN latest USING(sender_id,request_id,sequence)
        WHERE n.receiver_id=? GROUP BY sender_id,request_id)
        SELECT n.payload FROM native_lifecycle_index_v1 n JOIN selected USING(event_key)
        ORDER BY n.sender_id,n.request_id LIMIT 1001''',(receiver,receiver)).fetchall()
    complete=len(rows)<=1000
    return dict(receiver_id=receiver,observations=[json.loads(row[0]) for row in rows[:1000]],complete=complete,
        warning='' if complete else 'Native request inventory exceeds 1,000 states; delivery is deferred until coverage is complete.')
