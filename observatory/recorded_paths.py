"""Explicit recorded file paths, without filesystem or workspace inference."""
from __future__ import annotations

import json

from .command_evidence import modifying_action
from .operation_facts import safe_value


def path_facts(event):
    action = modifying_action(event)
    result = dict(action=action, paths=[], complete=True, reasons=[])
    if not action:
        return result

    def add(value, role):
        if not isinstance(value, str) or not value:
            return
        text, complete = safe_value(value)
        if not complete or len(text.encode()) > 4096:
            result['complete'] = False
            result['reasons'].append('A recorded path was redacted or clipped')
            return
        item = dict(path=text, role=role)
        if item not in result['paths']:
            result['paths'].append(item)

    add(event.get('subject') or event.get('path'), 'recorded')
    if action == 'rename':
        # Only explicit rename fields qualify. An arrow can be part of a name.
        detail = event.get('detail')
        if isinstance(detail, str) and len(detail) <= 8000:
            try:
                detail = json.loads(detail)
            except (ValueError, RecursionError):
                detail = None
        values = [event]
        if isinstance(detail, dict):
            values.append(detail)
            for key in ('rename', 'move', 'Rename', 'Move'):
                if isinstance(detail.get(key), dict):
                    values.append(detail[key])
            if isinstance(detail.get('type'), dict):
                values.extend(value for key, value in detail['type'].items()
                              if key in {'rename', 'move', 'Rename', 'Move'} and isinstance(value, dict))
        for value in values:
            for key in ('from_path', 'old_path', 'from'):
                add(value.get(key), 'from')
            for key in ('to_path', 'new_path', 'to'):
                add(value.get(key), 'to')
        if not {'from', 'to'} <= {item['role'] for item in result['paths']}:
            result['complete'] = False
            result['reasons'].append('Both rename paths were not explicitly recorded')
    if not result['paths']:
        result['complete'] = False
        result['reasons'].append('No usable recorded path')
    result['reasons'] = list(dict.fromkeys(result['reasons']))
    return result
