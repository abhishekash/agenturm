"""Trusted Activity preview declaration for one requested Pi shell command.

The projected row is content-free. The local route alone decides whether the
selected event has a current, independently proved Pi command to display.
"""
from __future__ import annotations

from .plugin_activity import _EVENT_KEY
from .ui_plugin_content import ContentPreviewProvider


def eligible_requested_command(record) -> bool:
    """Select only a projected, started shell call with an operation identity."""
    if not isinstance(record, dict):
        return False
    metadata = record.get('metadata')
    operation_id = record.get('operation_id')
    return (record.get('source') == 'plugin'
            and record.get('domain') == 'tool'
            and record.get('action') == 'shell'
            and record.get('result') == 'started'
            and isinstance(record.get('id'), str)
            and bool(_EVENT_KEY.fullmatch(record['id']))
            and isinstance(operation_id, str)
            and operation_id.startswith('plugin-operation:')
            and len(operation_id) == len('plugin-operation:') + 64
            and all(char in '0123456789abcdef'
                    for char in operation_id[len('plugin-operation:'):])
            and isinstance(metadata, dict)
            and metadata.get('capture') == 'content-free'
            and metadata.get('native_operation_observed') is True
            and isinstance(metadata.get('plugin_source_id'), str)
            and bool(metadata['plugin_source_id']))


PI_REQUESTED_COMMAND = ContentPreviewProvider(
    'pi-requested-command', eligible_requested_command,
    '/api/plugin-command-content', 'Requested shell command',
    'z Check for a redacted requested command for this exact event.',
    '… Excerpt clipped; the requested command remains in the original Pi file.',
    '(empty requested command)')
