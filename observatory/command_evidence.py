"""Bounded interpretation of recorded validation commands. Never executes a shell."""

from __future__ import annotations
import re
import shlex

INFORMATIONAL = {
    "--help",
    "-h",
    "--version",
    "-V",
    "--collect-only",
    "--co",
    "--fixtures",
    "--fixtures-per-test",
    "--markers",
    "--trace-config",
    "--list",
    "--listTests",
    "--listFilesOnly",
    "--showConfig",
    "--show-files",
    "--show-settings",
    "--dry-run",
    "--if-present",
}
SHELLS = {"sh", "bash", "zsh", "dash", "ksh"}


def _unwrap(command):
    for _ in range(4):
        try:
            words = shlex.split(command)
        except ValueError:
            return None
        if not words:
            return None
        executable = words[0].rsplit("/", 1)[-1]
        if executable not in SHELLS:
            return command
        # Only a known -c/-lc style envelope with exactly one command operand.
        flags = words[1:-1]
        if (
            not flags
            or flags[-1] not in {"-c", "-lc", "-cl"}
            or any(
                flag not in {"-l", "--login", "--noprofile", "--norc"}
                for flag in flags[:-1]
            )
        ):
            return None
        command = words[-1]
    return None


def _kind(words):
    words = list(words)
    while words and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]):
        assignment = words.pop(0)
        name, value = assignment.split("=", 1)
        if name.casefold() in {"pytest_addopts", "npm_config_if_present"}:
            try:
                options = shlex.split(value)
            except ValueError:
                return None
            if name.casefold() == "npm_config_if_present" or any(
                option.split("=", 1)[0] in INFORMATIONAL for option in options
            ):
                return None
    if not words:
        return None
    if any(word.split("=", 1)[0] in INFORMATIONAL for word in words):
        return None
    if words[0] in ("uv", "poetry") and words[1:2] == ["run"]:
        words = words[2:]
        while words and words[0] in (
            "--no-sync",
            "--offline",
            "--frozen",
            "--locked",
            "--no-project",
            "--active",
        ):
            words.pop(0)
    if not words:
        return None
    executable = words[0].rsplit("/", 1)[-1]
    arguments = words[1:]
    if executable in ("jest", "vitest") and arguments[:1] == ["list"]:
        return None
    if executable in ("tsc", "eslint") and "-v" in arguments:
        return None
    if executable == "ruff" and not (
        arguments[:1] == ["check"]
        or arguments[:1] == ["format"]
        and "--check" in arguments
    ):
        return None
    if executable == "go" and any(
        word == "-list" or word.startswith("-list=") for word in arguments
    ):
        return None
    if executable in ("make", "go") and "-n" in arguments:
        return None
    if executable in (
        "pytest",
        "ruff",
        "mypy",
        "pyright",
        "tsc",
        "eslint",
        "flake8",
        "pylint",
        "jest",
        "vitest",
    ):
        return executable
    if re.fullmatch(r"python(?:\d+(?:\.\d+)*)?", executable) and words[1:3] in (
        ["-m", "unittest"],
        ["-m", "pytest"],
    ):
        return words[2]
    if executable in ("npm", "pnpm", "yarn", "bun") and len(words) > 1:
        action = words[2] if words[1] == "run" and len(words) > 2 else words[1]
        if re.fullmatch(
            r"(?:test|check|lint|typecheck|type-check)(?::[A-Za-z0-9:_-]+)?", action
        ):
            return action
    if executable in ("cargo", "go") and words[1:2] in (
        ["test"],
        ["check"],
        ["clippy"],
        ["vet"],
    ):
        return executable + " " + words[1]
    if executable == "make" and words[1:2] in (["test"], ["check"], ["lint"]):
        return "make " + words[1]
    if executable == "git" and words[1:2] == ["diff"] and "--check" in arguments:
        return "git diff --check"
    return None


def analyze_command(command):
    """Recognize simple validators and safe AND chains; retain uncertainty otherwise.

    A zero shell result establishes every AND segment completed successfully only
    when none can terminate/branch the shell early. Unknown segments are refused.
    """
    if not isinstance(command, str) or len(command) > 8000:
        return None
    command = _unwrap(command)
    if not command or any(
        token in command
        for token in ("||", ";", "|", "`", "$", "#", "\n", "\r", "<(", ">(")
    ):
        return None
    # Descriptor redirection does not create a background job. Strip only its
    # fixed numeric spelling before lexing control operators, never evaluate it.
    lexical = re.sub(r"(?<!\w)[012]?>&[012](?!\w)", " ", command)
    try:
        lexer = shlex.shlex(lexical, posix=True, punctuation_chars=";&|")
        lexer.whitespace_split = True
        lexer.commenters = ""
        tokens = list(lexer)
    except ValueError:
        return None
    if any(token in {"&", "|", "||", ";", ";&", "|&", ";;"} for token in tokens):
        return None
    groups = [[]]
    for token in tokens:
        if token == "&&":
            if not groups[-1]:
                return None
            groups.append([])
        else:
            groups[-1].append(token)
    if not groups[-1]:
        return None
    kinds = []
    for words in groups:
        kind = _kind(words)
        if kind:
            kinds.append(kind)
        elif len(groups) > 1 and words[0] == "cd" and len(words) > 1:
            continue
        else:
            return None
    if not kinds:
        return None
    return {
        "kind": " + ".join(dict.fromkeys(kinds)),
        "command": command,
        "compound": len(groups) > 1,
    }


def check_kind(command):
    observation = analyze_command(command)
    return observation["kind"] if observation else None


MODIFYING_ACTIONS = {
    "write": "write",
    "edit": "edit",
    "update": "edit",
    "add": "create",
    "delete": "delete",
    "remove": "delete",
    "create": "create",
    "patch": "patch",
    "rename": "rename",
}


def modifying_action(event):
    return (
        MODIFYING_ACTIONS.get(str(event.get("action") or "").casefold())
        if event.get("domain") == "file"
        else None
    )
