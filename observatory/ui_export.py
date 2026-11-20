"""Explicit, previewed exports of selected evidence, never receiver configuration."""
from __future__ import annotations

import json
import os
from pathlib import Path
from urllib.parse import urlencode

from rich.text import Text
from textual import on, work
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, RichLog, Static
from .ui_content import ContentLog

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


def current_plugin_activity_revision(api, record, receiver_id):
    """Match one frozen Activity row to the receiver's current source revision."""
    if (record.get('source') != 'plugin' or not receiver_id
            or record.get('receiver_id') != receiver_id
            or not all(record.get(key) for key in ('id', 'source_id', 'session_id',
                                                   'agent_id', 'service'))):
        return None
    try:
        response = api.get('/api/plugin-activity-proof?' + urlencode({'event_id': record['id']}))
    except Exception:
        return None
    proof = response.get('proof') if isinstance(response, dict) else None
    if (not isinstance(proof, dict) or response.get('receiver_id') != receiver_id
            or proof.get('receiver_id') != receiver_id
            or not isinstance(proof.get('record'), dict)):
        return None
    revision = proof.get('revision')
    if not (isinstance(revision, list) and len(revision) == 3
            and all(type(value) is int and value >= 0 for value in revision)):
        return None
    current = proof['record']
    if any(current.get(key) != record[key] for key in
           ('id', 'source_id', 'session_id', 'agent_id', 'service', 'receiver_id')):
        return None
    if investigation_json('activity', current) != investigation_json('activity', record):
        return None
    return tuple(revision)


def write_investigation(path, content):
    """Create owner-only files; refuse to overwrite files or follow symlinks."""
    target = Path(path).expanduser()
    descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
            stream.write(content)
    except BaseException:
        # Do not leave a truncated export that could be mistaken for a complete one.
        try:
            os.close(descriptor)
        except OSError:
            pass
        try:
            target.unlink()
        except OSError:
            pass
        raise


class ExportScreen(ModalScreen):
    DEFAULT_CSS = '''
    ExportScreen { align: center middle; background: $background 70%; }
    #export-dialog { width: 90%; height: 90%; border: round $warning; padding: 1 2; background: $surface; }
    #export-warning { height: auto; margin-bottom: 1; }
    #export-preview { height: 1fr; }
    #export-path { height: 3; }
    #export-actions { height: 3; }
    #export-actions Button { width: 1fr; }
    '''
    BINDINGS = [('escape', 'dismiss', 'Cancel')]

    def __init__(self, view, record, scope=None, *, content=None, can_export=None,
                 poll_policy=True):
        super().__init__()
        self.content = content if content is not None else investigation_json(view, record, scope)
        self.can_export=can_export
        self.poll_policy=poll_policy
        self._export_invalidated=False

    def compose(self) -> ComposeResult:
        with Vertical(id='export-dialog'):
            yield Static('PRIVACY PREVIEW · Nothing has been written.\nThis exact JSON may contain prompts, outputs, paths, session/trace IDs, or secrets recorded by your agent. It is NOT redacted. Review before sharing. Only the selected record is included; no receiver credentials or background history are added.', id='export-warning', markup=False)
            yield ContentLog(id='export-preview', min_width=1, wrap=True, markup=False, auto_scroll=False)
            yield Input(placeholder='Destination file (must not already exist)', id='export-path')
            with Horizontal(id='export-actions'):
                yield Button('Cancel', id='export-cancel')
                yield Button('Write reviewed export', id='export-confirm', variant='warning')

    def on_mount(self):
        preview=self.query_one(RichLog)
        # Let RichLog defer measuring until layout; on_mount may have width 0.
        preview.write(Text(self.content))
        preview.scroll_home(animate=False)
        self.query_one('#export-cancel', Button).focus()
        if self.can_export is not None and self.poll_policy:
            self.set_interval(.25,self.check_export_policy)

    @on(Button.Pressed, '#export-cancel')
    def cancel(self):
        self.dismiss()

    def action_copy_text(self):
        # Textual's Screen Ctrl+C/Super+C binding bypasses the Write button.
        # A process Activity preview must pass the exact source proof again.
        if self.check_export_policy():
            super().action_copy_text()

    @on(Button.Pressed, '#export-confirm')
    def confirm(self):
        if not self.can_write():return
        path = self.query_one('#export-path', Input).value.strip()
        if not path:
            self.notify('Choose a destination file first.', severity='warning')
            return
        self.query_one('#export-confirm',Button).disabled=True
        self.write_export(path)

    def check_export_policy(self):
        if self._export_invalidated or self.can_export is not None and not self.can_export():
            self._export_invalidated=True
            self.content=''
            self.query_one('#export-preview',RichLog).clear()
            self.query_one('#export-confirm',Button).disabled=True
            self.query_one('#export-warning',Static).update('Source content withheld: capture policy or receiver changed. Reopen current evidence before exporting.')
            return False
        return True

    def can_write(self):
        return self.check_export_policy()

    @work(thread=True,exclusive=True,exit_on_error=False)
    def write_export(self,path):
        error=''
        try:
            if self._export_invalidated or self.can_export is not None and not self.can_export():
                self._export_invalidated=True
                error='Source capture or receiver changed; nothing was written.'
            else:write_investigation(path,self.content)
        except FileExistsError:
            error='Destination already exists. Choose a new file; nothing was overwritten.'
        except (OSError, ValueError):
            error='Could not write export. Check the destination and permissions.'
        try:self.app.call_from_thread(self.finish_export,error)
        except RuntimeError:pass

    def finish_export(self,error):
        if not self.is_mounted:return
        self.query_one('#export-confirm',Button).disabled=self._export_invalidated
        if error:self.notify(error,severity='error');return
        self.dismiss()
        self.app.notify('Selected investigation exported. Review again before sharing.')


class ReceiptExportScreen(ExportScreen):
    """Preview the frozen Markdown; write its HTML companion from the same snapshot."""
    DEFAULT_CSS='''
    ReceiptExportScreen #export-dialog { width: 100%; height: 100%; padding: 0 1; }
    ReceiptExportScreen #export-warning { height: 3; margin-bottom: 0; }
    ReceiptExportScreen #export-actions Button { min-width: 0; }
    '''

    def __init__(self, formats, *, valid=None):
        super().__init__('review',{},content=formats['markdown'])
        self.formats=dict(formats)
        self.valid=valid

    def on_mount(self):
        super().on_mount()
        self.query_one('#export-warning',Static).update('Frozen evidence · not redacted. Review before sharing.\nWrites new private Markdown + HTML files; existing files are preserved. Esc Cancel · Tab Controls')
        self.query_one('#export-path',Input).placeholder='New receipt.md (or .html / .json)'
        self.query_one('#export-confirm',Button).label='Write MD + HTML'
        self.set_interval(.25,self.check_receipt_policy)

    def receipt_allowed(self):
        if self._export_invalidated:return False
        try:allowed=self.valid is None or self.valid()
        except Exception:allowed=False
        if not allowed:self._export_invalidated=True
        return allowed

    def action_copy_text(self):
        if self.check_receipt_policy():
            super().action_copy_text()

    def check_receipt_policy(self):
        if self.receipt_allowed():return True
        self.content='';self.formats={}
        self.query_one('#export-preview',RichLog).clear()
        self.query_one('#export-confirm',Button).disabled=True
        self.query_one('#export-warning',Static).update('Source policy or receiver changed. Reopen Review before exporting; this preview is invalid.')
        return False

    def can_write(self):
        if not self.check_receipt_policy():
            self.notify('Capture policy changed. Reopen Review before exporting.',severity='warning')
            self.dismiss();return False
        return True

    @work(thread=True,exclusive=True,exit_on_error=False)
    def write_export(self,path):
        from .work_receipts import export_receipt
        error=''
        try:
            if not self.receipt_allowed():error='Source policy or receiver changed; no receipt was written.'
            else:export_receipt(path,self.formats)
        except FileExistsError:error='A destination already exists. Choose a new name; nothing was overwritten.'
        except ValueError as exc:error=str(exc)
        except OSError:error='Could not write receipt. Check the destination and permissions.'
        try:self.app.call_from_thread(self.finish_export,error)
        except RuntimeError:pass

    def finish_export(self,error):
        if not self.is_mounted:return
        self.query_one('#export-confirm',Button).disabled=self._export_invalidated
        if error:self.notify(error,severity='error');return
        self.dismiss();self.app.notify('Frozen Markdown and HTML receipt exported. Review before sharing.')
