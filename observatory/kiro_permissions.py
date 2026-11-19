"""Read Kiro's standing permission rules without executing or changing them.

Kiro keeps rules in three places: the user file under ``<KIRO_HOME>/settings``,
a per-workspace file under ``<KIRO_HOME>/workspace-roots/<hash>`` (which also
records whether the folder is trusted), and a custom agent's ``permissions``.
Kiro writes ``permissions.yaml`` or ``permissions.json``. Only the small YAML
subset Kiro writes is accepted; anything else is reported as unreadable rather
than guessed at.
"""
from __future__ import annotations

import json
from pathlib import Path

MAX_FILE_BYTES = 64 * 1024
MAX_RULES = 100
MAX_PATTERNS = 50
MAX_TEXT = 200
FILENAMES = ("permissions.yaml", "permissions.json")


class Unreadable(ValueError):
    pass


def _scalar(text):
    text = text.strip()
    if not text:
        return ""
    if text[0] in "\"'":
        quote = text[0]
        if len(text) < 2 or text[-1] != quote:
            raise Unreadable("unterminated quoted value")
        body = text[1:-1]
        if quote == '"':
            try:
                return json.loads(text)
            except ValueError as exc:
                raise Unreadable("invalid quoted value") from exc
        return body.replace("''", "'")
    if text.startswith("[") and text.endswith("]"):
        inner = text[1:-1].strip()
        return [_scalar(item) for item in _split_flow(inner)] if inner else []
    if text in {"{}", "[]"}:
        return {} if text == "{}" else []
    if text[0] in "{&*!|>%@`":
        raise Unreadable("unsupported YAML value")
    if " #" in text:
        text = text.split(" #", 1)[0].rstrip()
    return {"true": True, "false": False, "null": None, "~": None}.get(text, text)


def _split_flow(text):
    items, current, quote = [], [], ""
    for char in text:
        if quote:
            current.append(char)
            if char == quote:
                quote = ""
        elif char in "\"'":
            quote = char
            current.append(char)
        elif char == ",":
            items.append("".join(current))
            current = []
        elif char in "[]{}":
            raise Unreadable("nested flow collection")
        else:
            current.append(char)
    if quote:
        raise Unreadable("unterminated quoted value")
    items.append("".join(current))
    return items


def _key_value(text):
    if ":" not in text:
        raise Unreadable("expected key: value")
    if text[0] in "\"'":
        raise Unreadable("quoted keys are not supported")
    key, _, value = text.partition(":")
    key = key.strip()
    if not key or value and not value.startswith(" "):
        raise Unreadable("expected key: value")
    return key, value.strip()


def parse_yaml(text):
    """Parse the block-mapping/sequence subset Kiro writes for permissions."""
    lines = []
    for raw in text.splitlines():
        if "\t" in raw[:len(raw) - len(raw.lstrip())]:
            raise Unreadable("tab indentation")
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped in {"---", "..."}:
            continue
        lines.append((len(raw) - len(raw.lstrip(" ")), stripped))
    position = 0

    def block(indent):
        nonlocal position
        if position >= len(lines):
            return None
        if lines[position][1].startswith("- ") or lines[position][1] == "-":
            return sequence(lines[position][0])
        return mapping(lines[position][0])

    def mapping(indent):
        nonlocal position
        result = {}
        while position < len(lines):
            level, text = lines[position]
            if level < indent:
                break
            if level > indent or text.startswith("-"):
                raise Unreadable("unexpected indentation")
            key, value = _key_value(text)
            position += 1
            if value:
                result[key] = _scalar(value)
            elif position < len(lines) and (lines[position][0] > indent or
                                            lines[position][0] == indent and lines[position][1].startswith("- ")):
                result[key] = block(lines[position][0])
            else:
                result[key] = None
        return result

    def sequence(indent):
        nonlocal position
        result = []
        while position < len(lines):
            level, text = lines[position]
            if level < indent:
                break
            if level > indent or not (text.startswith("- ") or text == "-"):
                raise Unreadable("unexpected sequence indentation")
            item = text[1:].strip()
            if not item:
                position += 1
                result.append(block(lines[position][0]) if position < len(lines) and lines[position][0] > indent else None)
            elif ":" in item and not item[0] in "\"'[" and _looks_like_key(item):
                # "- key: value" opens a mapping indented at the item text.
                lines[position] = (indent + 2, item)
                result.append(mapping(indent + 2))
            else:
                position += 1
                result.append(_scalar(item))
        return result

    value = block(0)
    if position != len(lines):
        raise Unreadable("unparsed content")
    return value


def _looks_like_key(item):
    key, _, rest = item.partition(":")
    return bool(key.strip()) and " " not in key.strip() and (not rest or rest.startswith(" "))


def _text(value):
    return str(value)[:MAX_TEXT]


def _rules(document):
    if document is None:
        return [], None
    if not isinstance(document, dict):
        raise Unreadable("not a permissions document")
    rules = document.get("rules")
    if rules is None:
        rules = []
    if not isinstance(rules, list):
        raise Unreadable("rules is not a list")
    result = []
    for rule in rules[:MAX_RULES]:
        if not isinstance(rule, dict):
            raise Unreadable("rule is not a mapping")
        item = {"capability": _text(rule.get("capability") or "any"),
                "effect": _text(rule.get("effect") or "unspecified")}
        for key in ("match", "exclude"):
            value = rule.get(key)
            if isinstance(value, str):
                value = [value]
            if isinstance(value, list):
                item[key] = [_text(entry) for entry in value[:MAX_PATTERNS] if entry is not None]
        result.append(item)
    trusted = document.get("workspaceTrusted")
    return result, trusted if isinstance(trusted, bool) else None


def read_rules(path):
    """Return {status, rules, trusted, truncated} for one rules file."""
    path = Path(path)
    try:
        stat = path.stat()
    except FileNotFoundError:
        return {"status": "absent", "rules": [], "trusted": None}
    except OSError:
        return {"status": "unreadable", "rules": [], "trusted": None, "reason": "cannot be opened"}
    if stat.st_size > MAX_FILE_BYTES:
        return {"status": "unreadable", "rules": [], "trusted": None, "reason": "larger than 64 KiB"}
    try:
        text = path.read_bytes().decode("utf-8")
        document = json.loads(text) if path.suffix == ".json" else parse_yaml(text)
        if path.suffix == ".json" and path.name != "permissions.json":
            document = document.get("permissions") if isinstance(document, dict) else None
        rules, trusted = _rules(document)
    except (OSError, UnicodeError, ValueError, RecursionError) as exc:
        return {"status": "unreadable", "rules": [], "trusted": None, "reason": str(exc)[:120]}
    count = len(document.get("rules") or []) if isinstance(document, dict) and isinstance(document.get("rules"), list) else 0
    return {"status": "read", "rules": rules, "trusted": trusted, "truncated": count > MAX_RULES}


def rules_file(directory):
    """Kiro prefers a non-empty permissions.yaml, then permissions.json."""
    directory = Path(directory)
    for name in FILENAMES:
        candidate = directory / name
        try:
            if candidate.is_file() and candidate.read_bytes().strip():
                return candidate
        except OSError:
            return candidate
    return directory / FILENAMES[0]


def display_path(path, home):
    try:
        return "~/" + str(Path(path).relative_to(home))
    except ValueError:
        return str(path)


def standing_sources(kiro_home, workspace_key, agent, workspace=None):
    """Paths of the rule sources that apply to one session, in display order."""
    kiro_home = Path(kiro_home)
    sources = [("User", rules_file(kiro_home / "settings"))]
    if workspace_key:
        sources.append(("Workspace", rules_file(kiro_home / "workspace-roots" / workspace_key)))
    if agent and agent != "vibe" and "/" not in agent and "\\" not in agent and agent not in {".", ".."}:
        # A workspace agent definition takes precedence over the user one.
        local = Path(workspace) / ".kiro" / "agents" / (agent + ".json") if workspace else None
        sources.append(("Agent " + agent, local if local is not None and local.is_file()
                        else kiro_home / "agents" / (agent + ".json")))
    return sources


MAX_WORKSPACES = 200


def _workspace_folder(kiro_home, key):
    """A workspace key is also the session directory name; its session.json names the folder."""
    directory = Path(kiro_home) / "sessions" / key
    try:
        candidates = sorted((entry for entry in directory.iterdir() if entry.name.startswith("sess_")),
                            key=lambda entry: entry.stat().st_mtime, reverse=True)[:3]
    except OSError:
        return None
    for candidate in candidates:
        try:
            path = candidate / "session.json"
            if path.stat().st_size > MAX_FILE_BYTES:
                continue
            data = json.loads(path.read_bytes())
        except (OSError, ValueError):
            continue
        paths = data.get("workspacePaths") or data.get("rootPaths") if isinstance(data, dict) else None
        if isinstance(paths, list) and len(paths) == 1 and isinstance(paths[0], str):
            return paths[0]
    return None


def workspace_permissions(kiro_home, cache):
    """Every per-folder rules file: its folder (when recorded), trust and rule count."""
    root = Path(kiro_home) / "workspace-roots"
    result = []
    try:
        entries = sorted(entry for entry in root.iterdir() if entry.is_dir())[:MAX_WORKSPACES]
    except OSError:
        return result
    for entry in entries:
        path = rules_file(entry)
        try:
            stat = path.stat()
            signature = (stat.st_mtime_ns, stat.st_size)
        except OSError:
            continue
        cached = cache.get(entry.name)
        if cached is None or cached[0] != signature:
            cached = (signature, read_rules(path), _workspace_folder(kiro_home, entry.name))
            cache[entry.name] = cached
        _, rules, folder = cached
        if rules.get("trusted") is not True and not rules.get("rules") and rules.get("status") == "read":
            continue
        result.append({"key": entry.name, "folder": folder, "trusted": rules.get("trusted"),
                       "status": rules.get("status"), "rule_count": len(rules.get("rules") or [])})
    return result
