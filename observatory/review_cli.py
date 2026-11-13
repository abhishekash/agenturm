"""Noninteractive exact-run evidence inspection; shares the cockpit projection."""
from __future__ import annotations

import os
import sys
import time
from types import SimpleNamespace

from .review_facts import refreshed_review_item, review_capture_policy
from .workflow_evidence import ReviewHistory, scope_for, render_review
from .work_receipts import freeze_receipt, export_receipt


def add_review_parser(sub):
    parser=sub.add_parser('review',help='inspect recorded evidence for one exact agent run')
    parser.add_argument('--endpoint',default=os.environ.get('OBSERVATORY_ENDPOINT','http://127.0.0.1:14318'),help='existing receiver HTTP address; never starts a receiver')
    parser.add_argument('--service',required=True,help='exact producer/service name')
    parser.add_argument('--agent-id',default='',help='exact canonical agent identity; excludes workers and siblings')
    parser.add_argument('--session-id',default='',help='exact session identity; required if agent ID is unavailable')
    parser.add_argument('--trace-id',required=True,help='exact recorded run trace ID; never follows a newer run')
    parser.add_argument('--pages',type=int,choices=range(1,6),default=1,help='maximum 200-record pages to read (default: 1; maximum: 5)')
    parser.add_argument('--format',choices=('text','json','markdown','html'),default='text')
    parser.add_argument('-o','--output',help='new .md/.html/.json destination; always writes Markdown and HTML companions; captured text is not redacted')
    return parser


def collect_review(api, *, service, agent_id='', session_id='', trace_id, pages=1):
    """Bounded reads, namespace binding and no fallback to a broader scope."""
    if not service or not trace_id or not (agent_id or session_id):
        raise ValueError('Provide --service, --trace-id and --agent-id or --session-id. Find identities in the session inspector Details or a Review JSON export.')
    if not 1<=pages<=5:raise ValueError('Read between 1 and 5 pages.')
    health=api.get('/api/health')
    receiver=health.get('receiver_id') if isinstance(health,dict) else None
    if not receiver:raise ValueError('Receiver has no stable identity; update it before exact-run review.')
    if health.get('review_trace_filter') is not True:raise ValueError('Receiver cannot filter an exact trace; update it before review.')
    local=health.get('local_agents') or {}
    item=dict(service=service,agent_id=agent_id,session_id=session_id,trace_id=trace_id,state='Unknown',task='Selected execution')
    snapshot=SimpleNamespace(receiver_id=receiver,local_status=local,sessions=[],traces=[],updated=time.time(),error='',read_errors={},sessions_error='')
    scope=scope_for(item,snapshot)
    # Capture policy applies to the selected identity even when an older run is requested.
    native=next((row for row in local.get('sessions',()) if row.get('service')==service and
        (row.get('agent_id')==agent_id if agent_id else not row.get('agent_id') and row.get('session_id')==session_id) and
        (not session_id or row.get('session_id')==session_id)),{})
    if native:
        for key in ('task','capture_content','capture_revision'): 
            if key in native:item[key]=native[key]
    if local.get('capture_content') is False:item['capture_content']=False
    if item.get('capture_content') is False:
        return ReviewHistory(scope,capture_revision=False,loaded=True).projection(item)
    sessions=api.get('/api/sessions?limit=100')
    if not isinstance(sessions,dict) or sessions.get('receiver_id')!=receiver or not isinstance(sessions.get('sessions'),list):
        raise ValueError('Session response identity changed or is unavailable; retry against the intended receiver.')
    snapshot.sessions=sessions['sessions']
    # An omitted optional session ID must not make matching native metadata disappear.
    if native and not session_id:item['session_id']=native.get('session_id') or ''
    scope=scope_for(item,snapshot)
    item=refreshed_review_item(item,snapshot,scope)
    history=ReviewHistory(scope,capture_revision=item.get('capture_revision',item.get('capture_content')))
    seen=set()
    for _ in range(pages):
        cursor=history.cursor
        response=api.get_review(scope.path(cursor))
        if not isinstance(response,dict) or response.get('receiver_id')!=receiver:
            raise ValueError('Activity response identity changed or is unavailable; no review emitted.')
        history.accept(response)
        if not history.cursor:break
        if history.cursor in seen:
            history.limited.append('Receiver repeated an activity cursor');history.cursor=None;break
        seen.add(history.cursor)
    after=api.get('/api/health')
    if not isinstance(after,dict) or after.get('receiver_id')!=receiver:
        raise ValueError('Receiver identity changed while reading evidence; no review emitted.')
    snapshot.local_status=after.get('local_agents') or {}
    policy=review_capture_policy(item,snapshot,scope)
    if policy.get('capture_revision')!=item.get('capture_revision') or policy.get('capture_content') is False:
        raise ValueError('Capture policy changed while reading evidence; retry to obtain a matching review.')
    return history.projection(item)


def run_review_command(args):
    from .terminal import Api, CockpitError
    try:
        projection=collect_review(Api(args.endpoint,os.environ.get('OBSERVATORY_TOKEN')),service=args.service,
            agent_id=args.agent_id,session_id=args.session_id,trace_id=args.trace_id,pages=args.pages)
        if args.output:
            paths=export_receipt(args.output,freeze_receipt(projection))
            print('Wrote '+', '.join(paths)+'. Captured content is not redacted.',file=sys.stderr)
        if args.format=='text':print(render_review(projection))
        else:sys.stdout.write(freeze_receipt(projection)[args.format])
        return 0
    except KeyboardInterrupt:return 130
    except (OSError,ValueError,TypeError,CockpitError) as exc:
        # HTTP exceptions can embed URLs and credentials from a remote error; keep diagnostics local.
        message=str(exc) if isinstance(exc,(ValueError,CockpitError,FileExistsError)) else 'Cannot read the receiver or write the destination; check the endpoint, token and file permissions.'
        print('Agenturm review: '+message,file=sys.stderr)
        return 2
