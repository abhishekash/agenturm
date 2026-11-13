"""Content-free attributes for internal approval-review executions."""
import math
from copy import deepcopy


def metadata_keys():
    from .core import SESSION_KEYS, INPUT_TOKEN_KEYS, OUTPUT_TOKEN_KEYS, TOKEN_BREAKDOWN_KEYS
    identifiers=set(SESSION_KEYS)|{'agent.id','agent.runtime','agent.namespace','agent.worker_id','agent.parent_id',
        'agent.turn.id','agent.run.state','agent.relationship.kind','agent.relationship.state',
        'agent.discovery_state','agent.coverage','observatory.source_id','observatory.record_id',
        'observatory.source.event_name','gen_ai.request.model','gen_ai.response.model',
        'gen_ai.operation.name','gen_ai.tool.type','gen_ai.tool.name','tool.name','tool.type',
        'mcp.method.name','mcp.server.name','mcp.tool.name','rpc.system',
        'agent.activity.domain','agent.activity.action','agent.activity.result','observatory.native_file_id'}
    numeric=set(INPUT_TOKEN_KEYS)|set(OUTPUT_TOKEN_KEYS)|{
        key for keys in TOKEN_BREAKDOWN_KEYS.values() for key in keys}|{
        'llm.cost.total','llm.cost.prompt','llm.cost.completion','llm.cost_usd','gen_ai.usage.cost','llm.cost_known',
        'observatory.native_usage','observatory.duration_unknown','duration_ms',
        'observatory.native_file_offset','observatory.native_mtime_ns'}
    return identifiers,numeric


def internal_review_attrs(attrs):
    identifiers,numeric=metadata_keys()
    attrs=attrs if isinstance(attrs,dict) else {}
    def number(value):
        return (isinstance(value,int) and 0<=value<=2**63-1 or
                isinstance(value,float) and math.isfinite(value) and 0<=value<=10**18)
    result={key:value for key,value in attrs.items() if
        key in identifiers and isinstance(value,str) and len(value)<=512 or
        key in numeric and number(value)}
    result['agent.activity.subject']='Approval review'
    return result


def internal_review_events(events):
    return []


def scrub_record(record,kind):
    result=deepcopy(record)
    result['service']='codex-review'
    result['attributes']=internal_review_attrs(result.get('attributes') or {})
    result.setdefault('resource_attributes',{})['service.name']='codex-review'
    if kind=='span':
        result.update(events=[],links=[])
        if result.get('name')=='agent.run':result['attributes']['agent.task']='Approval review'
        else:result['name']='Approval review'
    else:result['body']=''
    return result


def initialize(db):
    db.execute('CREATE TABLE IF NOT EXISTS native_source_policy_v1(source_id TEXT PRIMARY KEY,internal INTEGER NOT NULL)')
    identifiers,numeric=metadata_keys()
    identifiers|={'agent.task','agent.activity.subject'}
    def quoted(values):return ','.join("'"+key+"'" for key in sorted(values))
    invalid=f"""EXISTS(SELECT 1 FROM json_each(NEW.attributes) field WHERE
        field.key NOT IN ({quoted(identifiers|numeric)}) OR
        (field.key IN ({quoted(identifiers)}) AND (field.type!='text' OR length(field.value)>512)) OR
        (field.key IN ({quoted(numeric)}) AND field.type NOT IN ('integer','real','true','false')))
        OR COALESCE(json_extract(NEW.attributes,'$."agent.task"'),'Approval review')!='Approval review'
        OR COALESCE(json_extract(NEW.attributes,'$."agent.activity.subject"'),'Approval review')!='Approval review'"""
    for table in ('spans','logs'):
        order_guard=native_update_guard('OLD').replace('excluded.','NEW.')
        db.execute(f'''CREATE TRIGGER IF NOT EXISTS {table}_native_position_v1
            BEFORE UPDATE ON {table} WHEN NOT({order_guard})
            BEGIN SELECT RAISE(IGNORE); END''')
        content="NEW.body!='\"\"'" if table=='logs' else "NEW.events!='[]' OR NEW.links!='[]' OR NEW.name NOT IN ('agent.run','Approval review')"
        for operation in ('INSERT','UPDATE'):
            db.execute(f'''CREATE TRIGGER IF NOT EXISTS {table}_internal_policy_{operation.lower()}_v1
                BEFORE {operation} ON {table}
                WHEN EXISTS(SELECT 1 FROM native_source_policy_v1
                    WHERE source_id=json_extract(NEW.resource_attributes,'$."observatory.source_id"') AND internal=1)
                BEGIN SELECT CASE WHEN NEW.service!='codex-review' OR {content} OR {invalid}
                    THEN RAISE(ABORT,'Internal source requires metadata-only capture; update the collector') END; END''')


def native_update_guard(table,retry=False):
    def value(row,key):return f"json_extract({row}.attributes,'$.\"observatory.native_{key}\"')"
    oldfile,newfile=value(table,'file_id'),value('excluded','file_id')
    oldoffset,newoffset=(f'COALESCE({value(row,"file_offset")},-1)' for row in (table,'excluded'))
    oldmtime,newmtime=(f'COALESCE({value(row,"mtime_ns")},-1)' for row in (table,'excluded'))
    return f"""NOT({table}.scope_name='observatory.local_agents' AND excluded.scope_name={table}.scope_name
        AND json_extract({table}.resource_attributes,'$."observatory.source_id"')=json_extract(excluded.resource_attributes,'$."observatory.source_id"')
        AND {oldfile} IS NOT NULL AND ({newfile} IS NULL OR
            ({newfile}={oldfile} AND ({newoffset}<{oldoffset} OR ({newoffset}={oldoffset} AND {newmtime}<{oldmtime})))
            OR ({newfile}!={oldfile} AND {int(bool(retry))}=1)))"""
