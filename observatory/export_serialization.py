"""Dependency-light serializers for selected investigation exports."""
from __future__ import annotations

import json


FIELDS = {
    'agents': ('runtime', 'service', 'session_id', 'project', 'model', 'task', 'state', 'waiting_for',
               'agent_id', 'parent_agent_id', 'relationship_kind', 'relationship_state',
               'relationship_provenance', 'discovery_state', 'coverage'),
    'activity': ('id', 'service', 'agent_id', 'session_id', 'trace_id', 'span_id', 'turn_id',
                 'operation_id', 'source', 'source_id', 'time_unix_nano', 'domain', 'action',
                 'subject', 'result', 'detail', 'duration_ms', 'presentation', 'context_blocks'),
    'mcp': ('id', 'service', 'agent_id', 'session_id', 'trace_id', 'span_id', 'time_unix_nano', 'server', 'tool', 'method', 'status', 'arguments', 'output', 'duration_ms'),
    'traces': ('trace_id', 'summary', 'spans', 'logs', 'span_page', 'pagination',
               'evidence_window', 'findings', 'progress', 'outcome'),
    'metrics': ('service', 'name', 'kind', 'temporality', 'scope_name', 'unit', 'description', 'point_count'),
    'findings': ('receiver_id','producer','service','agent_id','turn_id','ownership_version',
                 'command','exit_code','output_kind','output_origin','result_basis','detail_clipped',
                 'evidence_complete','trace_count','session_count','operation_reference','operation_version',
                 'operation_result','operation_result_conflict','operation_expected_nonzero',
                 'operation_fingerprint','error_fingerprint','id', 'severity', 'kind', 'title', 'detail', 'task', 'session_id', 'trace_id', 'services', 'evidence', 'occurrence_count', 'time_unix_nano'),
}


_TOOL_TURN_RELATIONS = frozenset({
    'matched_call', 'ambiguous_call', 'unverified_native_turn', 'missing_call',
    'future_call', 'unproven_order', 'call_turn_unavailable', 'conflicting_turn',
})


def plugin_activity_provenance(record):
    """Export only receiver-derived, typed process facts; never arbitrary metadata."""
    if record.get('source') != 'plugin':
        return None
    metadata = record.get('metadata')
    if not isinstance(metadata, dict):
        metadata = {}
    provenance = {
        'capture': 'content-free',
        'coverage': 'partial',
        'snapshot_note': ('Frozen loaded Activity row. The cockpit rechecks source policy '
                          'before writing; a saved JSON file cannot be revalidated.'),
    }
    source_id = metadata.get('plugin_source_id')
    if isinstance(source_id, str) and 0 < len(source_id) <= 256 and source_id.isascii() \
            and all(character.isalnum() or character in '._:-' for character in source_id):
        provenance['plugin_source_id'] = source_id
    for key in ('native_worker_observed', 'native_turn_observed',
                'native_operation_observed', 'tool_category_conflict'):
        if type(metadata.get(key)) is bool:
            provenance[key] = metadata[key]
    relation = metadata.get('tool_turn_relation')
    if type(relation) is str and relation in _TOOL_TURN_RELATIONS:
        provenance['tool_turn_relation'] = relation
    return provenance


def investigation_json(view, record, scope=None):
    """Freeze the exact selected payload before showing the confirmation."""
    evidence = {key: record[key] for key in FIELDS.get(view, ()) if key in record}
    if view == 'activity':
        provenance = plugin_activity_provenance(record)
        if provenance is not None:
            evidence['source_provenance'] = provenance
    payload = {'format': 'agenturm-investigation-v1', 'view': view,
               'coverage': 'Selected record and currently loaded evidence only; not a complete history export.',
               'evidence': evidence}
    if scope:
        payload['hierarchy_scope'] = {
            'mode': scope.get('mode') or 'exact',
            'root_agent_id': scope.get('root_agent_id'),
            'note': 'Descendant evidence is included only when loaded by the active scoped query; this is not a family-wide export.',
        }
    return json.dumps(payload, ensure_ascii=False, indent=2) + '\n'


def review_investigation_json(projection, pi_user_text=None):
    """Serialize a Review snapshot, optionally with one exact Pi user message.

    The optional text is transient delivery data: callers must provide it only
    after the Pi reference reader accepted an exact source proof. It is never
    added to Activity, evidence history, or the ordinary Review projection.
    """
    payload = {'format': 'agenturm-investigation-v1', 'view': 'review',
               'coverage': 'Selected Review and currently loaded evidence only.',
               'evidence': projection}
    if pi_user_text is not None:
        if (not isinstance(pi_user_text, dict) or
                not isinstance(pi_user_text.get('source'), dict) or
                not isinstance(pi_user_text.get('text'), str) or
                not isinstance(pi_user_text.get('revision'), (list, tuple)) or
                len(pi_user_text['revision']) != 3):
            raise ValueError('invalid exact Pi user-text reference')
        from .plugin_activity import _EVENT_KEY
        from .workflow_evidence import safe_text
        source=pi_user_text['source'];text=pi_user_text['text']
        if (not isinstance(source.get('id'),str) or not _EVENT_KEY.fullmatch(source['id']) or
                source.get('source_id')!=source.get('id') or
                not all(isinstance(source.get(key),str) and source[key]
                        for key in ('service','session_id','receiver_id')) or
                safe_text(text)!=text or len(text.encode('utf-8'))>4096 or
                any(type(value) is not int or value<0 for value in pi_user_text['revision'])):
            raise ValueError('invalid exact Pi user-text reference')
        payload['pi_user_text'] = {
            'source': pi_user_text['source'],
            'redacted_text': pi_user_text['text'],
            'revision': list(pi_user_text['revision']),
            'capture': 'opt-in source-backed redacted text',
        }
    return json.dumps(payload, ensure_ascii=False, indent=2) + '\n'

