"""Kiro-owned permission facts for the trusted session inspector.

The collector already redacts folder paths and subjects for metadata-only
capture. This provider renders only the current snapshot; it keeps no cache.
"""
from __future__ import annotations

import os
import time

from .ui_session_inspector import SessionInspectorModel, SessionInspectorProvider

PERMISSION_LABELS = {"allow_once": "Allowed once", "allow_always": "Always allowed",
                     "reject_once": "Denied", "reject_always": "Always denied",
                     "cancelled": "Cancelled", "pending": "Waiting", "answered": "Answered",
                     "policy_denied": "Blocked by rule"}


def _home_relative(path):
    home = os.path.expanduser('~')
    text = str(path or '')
    return '~' + text[len(home):] if home and (text == home or text.startswith(home + os.sep)) else text


def _rule_line(rule):
    effect = str(rule.get('effect') or 'unspecified')
    capability = str(rule.get('capability') or 'any')
    match = rule.get('match') if isinstance(rule.get('match'), list) else None
    line = f'{effect} {capability}: ' + (', '.join(map(str, match[:8])) + (f' (+{len(match)-8})' if len(match) > 8 else '')
                                         if match else 'anything')
    exclude = rule.get('exclude') if isinstance(rule.get('exclude'), list) else None
    if exclude:
        line += ' · except ' + ', '.join(map(str, exclude[:4]))
    return line


def standing_rules_text(data, folders=()):
    lines = []
    rules = data.get('rules') if isinstance(data.get('rules'), list) else []
    if rules:
        lines += ['', 'Standing rules for this session (Kiro checks these before asking)']
    for source in rules:
        if not isinstance(source, dict):
            continue
        label = str(source.get('source') or 'Rules')
        where = str(source.get('path') or '')
        if label == 'Workspace' and source.get('folder'):
            trust = ('trusted' if source.get('trusted') is True else 'not trusted' if source.get('trusted') is False
                     else 'trust not recorded')
            label += ' ' + _home_relative(source['folder']) + ' · ' + trust
        status = source.get('status')
        entries = source.get('rules') or []
        if status == 'absent':
            lines.append(f'  {label} · no rules file')
            continue
        if status != 'read':
            lines.append(f'  {label} · {where} could not be read' + (f" ({source['reason']})" if source.get('reason') else ''))
            continue
        lines.append(f'  {label} · {where}' + ('' if entries else ' · no rules'))
        lines += ['    ' + _rule_line(rule) for rule in entries if isinstance(rule, dict)]
        if source.get('truncated'):
            lines.append('    more rules not shown')
    trusted = [item for item in folders or () if isinstance(item, dict) and item.get('trusted') is True]
    if trusted:
        current = next((str(source.get('folder')) for source in rules
                        if isinstance(source, dict) and source.get('source') == 'Workspace'), '')
        lines += ['', f'Trusted folders ({len(trusted)})']
        for item in trusted[:20]:
            folder = item.get('folder')
            name = _home_relative(folder) if folder else 'folder not recorded · workspace-roots/' + str(item.get('key'))
            extra = ' · this session' if folder and folder == current else ''
            if item.get('rule_count'):
                extra += f" · {item['rule_count']} rule" + ('s' if item['rule_count'] != 1 else '')
            lines.append('  ' + name + extra)
        if len(trusted) > 20:
            lines.append(f'  and {len(trusted)-20} more')
    return lines


def permission_summary(item, folders=()):
    """(overview line, details text) for Kiro's recorded tool-approval answers."""
    data = item.get('permissions') if isinstance(item, dict) else None
    if not isinstance(data, dict):
        return '', ''
    counts = data.get('counts') if isinstance(data.get('counts'), dict) else {}
    count = lambda key: counts.get(key) if type(counts.get(key)) is int else 0
    autopilot = data.get('autopilot')
    pilot = 'autopilot on' if autopilot is True else 'autopilot off' if autopilot is False else ''
    requested = count('requested')
    order = ('allow_always', 'allow_once', 'reject_always', 'reject_once', 'cancelled', 'answered')
    answered = sum(count(key) for key in order)
    parts = [f'{count(key)} {PERMISSION_LABELS[key].lower()}' for key in order if count(key)]
    if requested > answered:
        parts.append(f'{requested-answered} waiting')
    blocked = count('policy_denied')
    tail = ''.join(' · ' + part for part in (
        f'{blocked} blocked by rule' if blocked else '', pilot,
        next(('trusted folder' if source.get('trusted') is True else 'untrusted folder' if source.get('trusted') is False else ''
              for source in data.get('rules') or () if isinstance(source, dict) and source.get('source') == 'Workspace'), ''))
        if part)
    if not requested:
        short = 'No approval requests recorded' + tail
    else:
        short = f'{requested} request' + ('s' if requested != 1 else '') + ': ' + ', '.join(parts) + tail
    lines = [short]
    for entry in data.get('recent') or []:
        if not isinstance(entry, dict):
            continue
        stamp = entry.get('requested_ns')
        when = time.strftime('%H:%M', time.localtime(stamp / 1e9)) if type(stamp) is int and stamp > 0 else '--:--'
        decision = PERMISSION_LABELS.get(entry.get('decision'), str(entry.get('decision') or 'Unknown'))
        what = ' · '.join(str(entry[key]) for key in ('capability', 'tool') if entry.get(key)) or 'tool'
        line = f'{when}  {decision:<14} {what}'
        if entry.get('subject'):
            line += '  ' + ' '.join(str(entry['subject']).split())[:100]
        resolved = entry.get('resolved_ns')
        if type(stamp) is int and type(resolved) is int and resolved >= stamp > 0:
            line += f'  (answered after {max(0, resolved-stamp)/1e9:.0f}s)'
        lines.append(line)
    if requested > len(data.get('recent') or []):
        lines.append(f'Showing the latest {len(data.get("recent") or [])}.')
    if data.get('complete') is False:
        lines.append('Counts cover only the part of this session the collector read.')
    lines.append('Answers Kiro recorded for tool-approval requests in this session.')
    lines += standing_rules_text(data, folders)
    return short, '\n'.join(lines)


def _matches(item: dict) -> bool:
    return item.get("runtime") == "Kiro" and isinstance(item.get("permissions"), dict)


def _render(item: dict, local_status: dict) -> SessionInspectorModel:
    short, details = permission_summary(item, local_status.get("kiro_folders") or ())
    return SessionInspectorModel(
        overview_fields=(("Permissions", short),) if short else (),
        detail_sections=(("Permissions", details),) if details else (),
    )


KIRO_PERMISSIONS = SessionInspectorProvider("kiro-permissions", _matches, _render)
