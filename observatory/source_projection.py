"""UI-independent session identity, source preparation and health projection.

Attention consumers share these exact ownership/capture/status semantics. No
terminal framework or filesystem work is needed to prepare a source snapshot.
"""
from copy import deepcopy
import time
import weakref

from .agent_hierarchy import AGENT_ID,canonical_agent_id
from .session_status import session_health,session_phase
from .session_prompts import prompt_scope,scoped_events,scoped_prompts
from .session_text import message_text,task_description
from .workflow_evidence import scope_for

def recorded_message_text(event):
    """Extract text already present in a recorded user-message event."""
    return (message_text(event.get('detail')) or message_text(event.get('subject')) or
            'Recorded request text unavailable')

def review_session(entry):
    """Recognize internal reviews in native, connection, and stored summaries."""
    services=[entry.get('service'),*(entry.get('services') or [])]
    models=[entry.get('model'),*(entry.get('models') or [])]
    return (bool(entry.get('review')) or 'codex-review' in services or
            'codex-auto-review' in models)

def unique_agents(items):
    """Count and draw a canonical agent once; prefer observed over announced."""
    by_agent={}
    for item in items:
        identity=item.get('agent_id') or item['id']
        previous=by_agent.get(identity)
        if previous is None or (previous.get('discovery_state')=='announced' and
                                item.get('discovery_state')!='announced'):
            by_agent[identity]=item
    return list(by_agent.values())

def prepare_sources(snapshot, include_reviews=False, *, denied_sources=frozenset(), policy_snapshot=None):
    """Prepare attributed identity and text, without clock-dependent health."""
    health_inputs={}
    connections={item['source_id']:item for item in snapshot.agents}
    by_agent={};by_session={}
    for event in snapshot.activity:
        if event.get('agent_id'):by_agent.setdefault(event['agent_id'],[]).append(event)
        if event.get('session_id'):by_session.setdefault(event['session_id'],[]).append(event)
    result=[]
    for entry in snapshot.local_status.get('sessions',[]):
        connection=connections.pop(entry['source_id'],{})
        review=review_session(entry)
        if review and not include_reviews:
            continue
        health_inputs[entry['source_id']]=entry
        result.append({**entry,'id':entry['source_id'],'review':review,
                       'spans':connection.get('span_count',0)})
    for entry in connections.values():
        review=review_session(entry)
        if review and not include_reviews:
            continue
        if 'local-session-log' in entry.get('integrations',[]):
            continue
        result.append({**entry,'id':entry['source_id'],'session_id':None,'project':'SDK / OTLP','model':'',
                       'runtime':entry.get('service'),'task':'Task not reported','recent':entry['state']=='running',
                       'state':'Connected' if entry['state']=='running' else 'Finished' if entry['state']=='stopped' else 'Offline',
                       'stamp':entry.get('last_seen_ns',0),'spans':entry.get('span_count',0),'review':review,
                       'discovery_state':'observed','coverage':'unknown',
                       'relationship_kind':'unknown','relationship_state':'unresolved',
                       'relationship_provenance':'SDK/OTLP producer did not report agent hierarchy'})
    known_sessions={item.get('session_id') for item in result if item.get('session_id')}
    known_sessions.update(item.get('session_id') for item in snapshot.local_status.get('sessions',[]))
    known_agent_ids={item.get('agent_id') for item in result if item.get('agent_id')}
    known_agent_ids.update(item.get('agent_id') for item in snapshot.local_status.get('sessions',[]))
    for session in getattr(snapshot,'sessions',[]):
        session_id=session.get('session_id')
        if not session_id:
            continue
        stored_agent_id=session.get('agent_id')
        if stored_agent_id and stored_agent_id in known_agent_ids:
            continue
        if session_id in known_sessions and not stored_agent_id:
            continue
        review=review_session(session)
        if review and not include_reviews:
            continue
        known_sessions.add(session_id)
        if stored_agent_id:
            known_agent_ids.add(stored_agent_id)
        services=session.get('services') or []
        service=services[0] if len(services)==1 else ''
        stamp=session.get('last_unix_nano') or 0
        # Trace health counts tool failures; it is not the task's outcome.
        reported=session.get('latest_task_outcome')
        if reported is None:
            reported={'RUNNING':'running','WAITING':'waiting','FAILED':'failed',
                      'INCOMPLETE':'incomplete'}.get(session.get('latest_status'),'unknown')
        waiting_for=session.get('waiting_for') or ''
        if reported in {'completed','failed','interrupted','incomplete'}:waiting_for=''
        if waiting_for and reported=='unknown':reported='waiting'
        runtime='Gemini CLI' if any('gemini' in name.casefold() for name in services) else 'OpenTelemetry'
        source_id='otlp-session:'+str(session_id)+(':'+str(stored_agent_id) if stored_agent_id else '')
        health_inputs[source_id]={'run_state':reported,
            'waiting_for':waiting_for,'last_record_ns':stamp}
        result.append({'id':source_id,'source_id':source_id,
            'session_id':session_id,'runtime':runtime,'service':service,'project':session.get('project') or '',
            'working_directory':'','branch':'','model':', '.join(session.get('models') or []),
            'task':session.get('latest_task') or '',
             'initial_task':session.get('initial_task') or '',
             'run_state':reported,'task_outcome_ns':session.get('task_outcome_ns') or 0,
             'waiting_for':waiting_for,'last_activity':{},'capture_content':None,
             'spans':session.get('span_count',0),
             'trace_count':session.get('trace_count',0),'errors':session.get('errors',0),
             'failed':session.get('failed',0),'incomplete':session.get('incomplete',0),
             'latest_trace_id':session.get('latest_trace_id'),
             'status_complete':session.get('status_complete',True),'review':review,
             'source_namespace':session.get('agent_namespace') or service or runtime,
             'native_session_id':session_id,'native_worker_id':session.get('agent_worker_id'),
             'relationship_kind':session.get('relationship_kind') or 'unknown',
             'relationship_state':session.get('relationship_state') or 'unresolved',
             'relationship_provenance':session.get('relationship_provenance') or
                 'SDK/OTLP session has no explicit parent metadata',
             'discovery_state':'observed','coverage':session.get('coverage') or 'unknown',
             'agent_id':session.get('agent_id'),
             'parent_agent_id':session.get('parent_agent_id'),
             'agent_native_id':session.get('agent_native_id'),
             'parent_agent_native_id':session.get('parent_agent_native_id')})
    for item in result:
        runtime=item.get('runtime') or item.get('service') or 'Agent'
        namespace=item.get('source_namespace') or item.get('source_id') or item.get('service') or runtime
        if not item.get('agent_id') or not AGENT_ID.fullmatch(str(item.get('agent_id'))):
            item['agent_id_inferred']=True
            native=(item.get('agent_native_id') or item.get('native_worker_id') or
                    item.get('session_id') or item.get('id'))
            if native:
                item['agent_id']=(str(native).lower() if AGENT_ID.fullmatch(str(native))
                                  else canonical_agent_id(str(runtime),str(namespace),str(native)))
        explicit_parent=item.get('parent_agent_native_id')
        if explicit_parent and not item.get('parent_agent_id'):
            try:
                item['parent_agent_id']=(str(explicit_parent).lower()
                    if AGENT_ID.fullmatch(str(explicit_parent)) else
                    canonical_agent_id(str(runtime),str(namespace),str(explicit_parent)))
            except ValueError:
                item['parent_agent_id']=None
            if item.get('parent_agent_id') and item.get('relationship_state')=='unresolved':
                item['relationship_kind']='delegated'
                item['relationship_state']='confirmed'
                item['relationship_provenance']='SDK/OTLP explicit agent.parent_id'
        item.setdefault('relationship_kind','unknown')
        item.setdefault('relationship_state','unresolved')
        item.setdefault('relationship_provenance','No parent relationship was reported')
        item.setdefault('discovery_state','observed')
        item.setdefault('coverage','unknown')
        item.setdefault('source_namespace',str(namespace))
        item.setdefault('native_session_id',item.get('session_id'))
        if item['id'] in denied_sources:item['capture_content']=False
        if policy_snapshot is not None:
            from .review_facts import review_capture_policy
            item.update(review_capture_policy(item,policy_snapshot,scope_for(item,snapshot)))
        if item.get('capture_content') is False:
            # Fresh health may revoke capture while a failed Activity refresh
            # leaves old text in the snapshot. Never derive text from it.
            item['task']='';item['initial_task']=''
            item['last_activity']={key:value for key,value in (item.get('last_activity') or {}).items()
                if key in {'time_unix_nano','domain','action','result','id','source_id','trace_id','span_id'}}
            continue
        scope=prompt_scope(item)
        candidates=(by_agent.get(item.get('agent_id'),()) if not item.get('agent_id_inferred')
                    else by_session.get(item.get('session_id'),()))
        evidence=scoped_events(candidates,scope)
        title=task_description(item.get('initial_task') or '')
        if title:
            item['task']=title
            item['task_source']='initial recorded user request'
        else:
            prompts=sorted(scoped_prompts(evidence,prompt_scope(item)),
                           key=lambda event:event.get('time_unix_nano') or 0)
            title=next((title for event in prompts
                        if (title:=task_description(recorded_message_text(event)))), '')
            if title:
                item['task']=title
                item['task_source']='recorded user request'
        if item['id'].startswith('otlp-session:'):
            item['last_activity']=max(evidence,key=lambda event:event.get('time_unix_nano') or 0,default={})
    return result,health_inputs

class SourcePreparation:
    """Two bounded snapshot preparations shared by this app's consumers.

    Equality against detached inputs catches nested in-place changes as well as
    receiver/scope/policy replacement. Preparations never cross snapshot objects.
    Health, attention, review receipts and live sorting are never cached here.
    """
    def __init__(self):
        self.entries=[]

    def clear(self):
        self.entries.clear()

    @staticmethod
    def bounded_copy(value):
        """Admit and detach JSON-shaped inputs in one bounded traversal."""
        nodes=20000;characters=512000
        def clone(item,depth=0):
            nonlocal nodes,characters
            nodes-=1
            if nodes<0 or depth>24:raise ValueError('Preparation budget')
            if isinstance(item,str):
                characters-=len(item)
                if characters<0:raise ValueError('Preparation text budget')
                return item
            if isinstance(item,dict):
                if len(item)*2>nodes:raise ValueError('Preparation node budget')
                return {clone(key,depth+1):clone(child,depth+1) for key,child in item.items()}
            if isinstance(item,(tuple,list)):
                if len(item)>nodes:raise ValueError('Preparation node budget')
                copied=[clone(child,depth+1) for child in item]
                return tuple(copied) if isinstance(item,tuple) else copied
            if item is None or isinstance(item,(bool,int,float)):return item
            raise ValueError('Unsupported preparation value')
        try:return clone(value)
        except ValueError:return None

    @classmethod
    def within_budget(cls,value):
        return cls.bounded_copy((value,)) is not None

    def get(self,snapshot,include_reviews,denied_sources,policy_snapshot):
        inputs=(getattr(snapshot,'receiver_id',''),getattr(snapshot,'view_scope',()),
                snapshot.agents,snapshot.local_status.get('sessions',[]),
                getattr(snapshot,'sessions',[]),snapshot.activity,denied_sources)
        # Feed responses are already byte bounded. Also bound retained rows and
        # bypass unusually large custom feeds rather than growing a text cache.
        bounded=(not denied_sources and sum(len(rows) for rows in inputs[2:5])<=500
                 and len(inputs[5])<=200)
        if denied_sources:self.clear()
        for index,(owner,reviews,frozen,prepared) in enumerate(self.entries):
            if owner() is snapshot and reviews==include_reviews:
                if bounded and frozen==inputs:
                    self.entries.append(self.entries.pop(index))
                    return prepared,True
                self.entries.pop(index)
                break
        self.entries[:]=[entry for entry in self.entries if entry[0]() is not None]
        frozen=self.bounded_copy(inputs[:-1]) if bounded else None
        if frozen is None:
            return (prepare_sources(snapshot,include_reviews,denied_sources=denied_sources,
                                    policy_snapshot=policy_snapshot),False)
        frozen=(*frozen,denied_sources)
        from types import SimpleNamespace
        detached=SimpleNamespace(agents=frozen[2],local_status={'sessions':frozen[3]},
                                 sessions=frozen[4],activity=frozen[5])
        prepared=prepare_sources(detached,include_reviews)
        try:owner=weakref.ref(snapshot)
        except TypeError:return prepared,False
        self.entries.append((owner,include_reviews,frozen,prepared))
        del self.entries[:-2]
        return prepared,True

def sources(snapshot, include_reviews=False, *, preparation=None, policy_snapshot=None):
    policy=policy_snapshot if policy_snapshot is not None else snapshot
    same_receiver=getattr(policy,'receiver_id','')==getattr(snapshot,'receiver_id','')
    denied=frozenset(entry['source_id'] for entry in policy.local_status.get('sessions',[])
                     if entry.get('capture_content') is False) if same_receiver else frozenset()
    if same_receiver and policy.local_status.get('capture_content') is False:denied=denied|{None}
    if preparation is not None:
        prepared,retained=preparation.get(snapshot,include_reviews,denied,policy if denied else None)
    else:
        prepared=prepare_sources(snapshot,include_reviews,denied_sources=denied,
                                 policy_snapshot=policy if denied else None)
        retained=False
    result=deepcopy(prepared[0]) if retained else prepared[0]
    now=time.time_ns()
    for item in result:
        entry=prepared[1].get(item['id'])
        if entry is not None:
            if entry.get('discovery_state')=='announced':
                item.update(state='Spawn recorded',active=False,recent=False,
                            stamp=entry.get('last_record_ns') or 0,
                            status_evidence='Spawn recorded · status unavailable')
            else:item.update(session_health(entry,now))
        item['attention']=attention_required(item)
        item['owns_attention']=attention_required(item)
        item['_display_attention']=session_phase(item)=='attention'
    return sorted(result,key=lambda item:(not (item.get('active') or item.get('state')=='Waiting'),not item['recent'],-item['stamp'],item['id']))

def input_required(item):
    reason=str(item.get('waiting_for') or '').casefold()
    return item.get('state')=='Waiting' and any(
        marker in reason for marker in ('approval','permission','user input','input request'))

def attention_required(item):
    """Share the filter/header rule: explicit waits, possible stalls and failures."""
    return input_required(item) or item.get('state') in {
        'Possibly stalled','Error','Failed','Interrupted','Incomplete'
    }


def attention_items(snapshot):
    """Prepare the same observed agents as the roster, without a terminal UI."""
    return sources(snapshot)
