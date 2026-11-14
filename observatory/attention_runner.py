"""UI-independent attention observation, durable eligibility and delivery.

No terminal framework, agent execution or approval action lives in this module.
"""
from __future__ import annotations
import shutil
import subprocess
import sys
import time

from .attention import AttentionLedger,fresh_observation
from .review_facts import task_run_usage
from .workflow_evidence import safe_text


def prepared_item(item,snapshot):
    selected=dict(item)
    usage=task_run_usage(snapshot.sessions,item,snapshot.traces)
    if any(key in getattr(snapshot,'read_errors',{}) for key in ('sessions','traces')):usage={}
    if selected.get('trace_id') and usage.get('trace_id')!=selected['trace_id']:usage={}
    if usage.get('trace_id') and not selected.get('trace_id'):selected['trace_id']=usage['trace_id']
    if usage.get('trace_id') and not usage.get('start_unix_nano'):
        trace=next((row for row in snapshot.traces if row.get('trace_id')==usage['trace_id'] and
            (not row.get('service') or row['service']==item.get('service'))),{})
        usage={**usage,'start_unix_nano':trace.get('start_unix_nano')}
    owner=usage.get('scope_owner') or {}
    if usage.get('turn_id') and all(owner.get(key)==selected.get(target) for key,target in
            (('producer','service'),('agent_id','agent_id'),('session_id','session_id'))):selected['turn_id']=usage['turn_id']
    selected['_workflow_usage']=usage
    return selected


def inventory_ready(snapshot):
    """Bounded relevant inventory, not an exhaustive retained-history scan."""
    return not inventory_deferral_reason(snapshot)


def inventory_deferral_reason(snapshot):
    """Explain a delivery gate without exposing raw read errors or inventing counts."""
    local=getattr(snapshot,'local_status',{}) or {}
    stamp=getattr(snapshot,'receiver_time_ns',0)
    if not getattr(snapshot,'receiver_id',''):return 'receiver identity is unavailable'
    if type(stamp) is not int or not 0<stamp<2**63:return 'receiver clock metadata is unavailable'
    if getattr(snapshot,'indexing_ready',False) is not True:return 'receiver indexing is not ready'
    if not getattr(snapshot,'updated',0):return 'the first inventory has not arrived'
    if getattr(snapshot,'error',''):return 'the receiver read is unavailable'
    if getattr(snapshot,'sessions_error','') or getattr(snapshot,'read_errors',{}):return 'relevant inventory reads are incomplete'
    if getattr(snapshot,'native_lifecycle_warning',''):return 'native lifecycle evidence is incomplete'
    if local.get('enabled') and local.get('initial_poll_complete') is not True:return 'local agent discovery is incomplete'
    if local.get('backlog_bytes'):return 'local capture is catching up'
    if any((row.get('diagnostics') or {}).get('read_error') or
            (row.get('diagnostics') or {}).get('pending_storage_records') for row in local.get('sessions',())):
        return 'local source capture is incomplete'
    return ''


def comparable_source_clock(item):
    # Only native capture on the receiver host supplies this clock assertion.
    # SDK/user timestamps and time of this client's HTTP receipt are not proof.
    if 'request_clock' in item:return item['request_clock']=='receiver'
    return (
        item.get('service') in {'codex','claude','claude-code','opencode','pi'}
        and 'run_state' in item and bool(item.get('diagnostics')))


def desktop_notify(title,body):
    """Bounded desktop delivery; never a shell or a remediation command."""
    if sys.platform.startswith('linux') and shutil.which('notify-send'):
        command=['notify-send','--app-name=Agenturm','--',title,body]
    elif sys.platform=='darwin' and shutil.which('osascript'):
        command=['osascript','-e','on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run',title,body]
    else:return 'Desktop notifications unavailable on this system'
    try:
        subprocess.run(command,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3,check=True)
        return 'Desktop notification delivered'
    except (OSError,subprocess.SubprocessError):return 'Desktop notification delivery failed; attention remains pending'


class AttentionRunner:
    def __init__(self,ledger=None):
        self.ledger=ledger or AttentionLedger();self.latest=None;self.warning='';self.native_observations=[];self.native_warning=''

    def update(self,items,snapshot,limits,*,endpoint='',now=None,store=None):
        selected=[prepared_item(item,snapshot) for item in items]
        from .source_projection import native_snapshot
        from .native_lifecycle import merge_observations,overlay_observations
        current=store.native_observations(snapshot.receiver_id) if store else ()
        merged,note=merge_observations(snapshot.receiver_id,self.native_observations,current)
        snapshot=native_snapshot(snapshot,merged,note)
        self.native_observations=list(getattr(snapshot,'native_lifecycle',()))
        self.native_warning=getattr(snapshot,'native_lifecycle_warning','')
        selected,resolutions=overlay_observations(selected,self.native_observations,snapshot.receiver_id)
        self.latest=(selected,snapshot,dict(limits),endpoint)
        result=self.ledger.update(selected,snapshot,limits,now,endpoint=endpoint)
        for scope,kind,request,key in resolutions:
            from .attention import stable_id
            identity=stable_id(scope.scope_key,kind,'request:'+request)
            incident=self.ledger.incidents.get(identity)
            if incident and incident.execution.get('native_request_key')==key:
                self.ledger.resolved.add((scope.namespace,identity));self.ledger.incidents.pop(identity,None)
        return result

    def persist(self,store,*,memory=False,storage_error='',now=None):
        if store:
            snapshot=self.latest[1] if self.latest else None
            namespace=getattr(snapshot,'receiver_id','')
            inventory={namespace:snapshot.receiver_time_ns} if snapshot and inventory_ready(snapshot) else {}
            observed=self.ledger._observed_scopes or set()
            incidents=[incident for incident in self.ledger.incidents.values() if incident.persistent and incident.scope_key in observed]
            comparable={incident.id for incident in incidents if comparable_source_clock(self.ledger.items.get(incident.scope_key,{}))}
            result=store.observe(incidents,resolved=self.ledger.resolved,now=now,inventory=inventory,
                new_ids={incident.id for incident in self.ledger.new_incidents},comparable_ids=comparable)
            self.ledger.restore(result.records)
            for identity in result.unpersisted:
                self.ledger.records[identity]={**self.ledger.incidents[identity].to_record(),'memory_only':True}
            overflow=sum(bool(record.get('memory_only')) for identity,record in self.ledger.records.items() if identity in self.ledger.incidents)
            self.warning=(f'Workflow storage capacity reached; {overflow} incident(s) are memory only. Acknowledgement and delivery are unavailable for them.' if overflow else '')
        elif not memory:
            for identity,incident in self.ledger.incidents.items():self.ledger.records[identity]={**incident.to_record(),'memory_only':True}
            self.warning='Durable workflow state is unavailable: '+storage_error+'. Incidents remain visible; acknowledgements and alerts are disabled.'
        if not store:self.ledger.records={key:record for key,record in self.ledger.records.items() if key in self.ledger.incidents}
        self.ledger.render_rows(snapshot=self.latest[1] if self.latest else None)
        if self.native_warning:self.warning=' '.join(filter(None,(self.warning,self.native_warning)))
        return self.warning

    def delivery_candidates(self,*,now=None):
        if not self.latest or not inventory_ready(self.latest[1]):return []
        now=time.time() if now is None else now
        return [incident for incident in self.ledger.incidents.values()
            if self.ledger.records.get(incident.id,{}).get('attention_eligible') and incident.alertable
            and incident.scope_key in (self.ledger._observed_scopes or set())
            and fresh_observation(self.ledger.items.get(incident.scope_key,{}),self.latest[1],now)
            and (incident.execution.get('resolution_policy')!='matching-request' or
                self.ledger.items.get(incident.scope_key,{}).get('_native_request_key')==incident.execution.get('native_request_key'))]

    def deliver(self,store,incidents,*,notify=desktop_notify,retry=False,stopped=lambda:False):
        claimed=[]
        for incident in incidents:
            if stopped():break
            if store.claim(incident.namespace,incident.id,incident.scope_key,retry=retry):claimed.append(incident)
        if not claimed:return ''
        projects=[safe_text(self.ledger.items.get(incident.scope_key,{}).get('project') or 'Local agent').replace('\n',' ')[:40] for incident in claimed]
        title='Agenturm · '+', '.join(dict.fromkeys(projects))[:120]
        try:result=notify(title,str(len(claimed))+' recorded attention incident(s). Open Agenturm to inspect; respond in the native agent terminal.')
        except Exception:result='Desktop notification delivery failed; attention remains pending'
        status='delivered' if 'delivered' in result else 'unavailable' if 'unavailable' in result else 'failed'
        for incident in claimed:store.finish_delivery(incident.namespace,incident.id,status=status,detail=result)
        self.ledger.restore(store.load());self.ledger.render_rows(snapshot=self.latest[1] if self.latest else None)
        return result
