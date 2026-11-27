use super::{Event, clip, safe_text};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_COMMAND: usize = 8_000;

pub(super) struct Facts {
    pub command: String,
    pub command_complete: bool,
    pub result: &'static str,
    pub exit_code: Option<i32>,
    pub conflict: bool,
    pub basis: &'static str,
    pub output: String,
    pub output_origin: Option<String>,
    pub output_kind: Option<&'static str>,
    pub output_clipped: bool,
    pub informational: bool,
    pub fingerprint: Option<String>,
}

impl Facts {
    pub fn from_event(event: &Event) -> Self {
        let raw_detail;
        let qualified = match event.operation_evidence.as_ref() {
            Some(body) if body["version"].as_u64() == Some(1) => Some(body),
            _ if event.detail_role.as_deref() == Some("operation_v1") => {
                raw_detail = match event.detail.as_ref() {
                    Some(Value::String(text)) => serde_json::from_str::<Value>(text).ok(),
                    Some(value @ Value::Object(_)) => Some(value.clone()),
                    _ => None,
                };
                raw_detail
                    .as_ref()
                    .filter(|value| value["version"].as_u64() == Some(1))
            }
            _ => None,
        };
        let empty = Value::Null;
        let evidence = qualified.unwrap_or(&empty);
        let (commands, commands_valid) = entries(evidence.get("commands"), 2);
        let (inputs, inputs_valid) = entries(evidence.get("inputs"), 2);
        let mut candidates = BTreeSet::new();
        let mut exact_command = false;
        let mut commands_valid = commands_valid;
        for command in commands {
            match command[1].as_str().filter(|text| !text.trim().is_empty()) {
                Some(text) => {
                    candidates.insert(text);
                    exact_command |= command[0].as_str() != Some("subject");
                }
                None => commands_valid = false,
            }
        }
        if event.domain == "shell" {
            for input in &inputs {
                for field in ["cmd", "command"] {
                    if let Some(text) = input[1]
                        .get(field)
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        candidates.insert(text);
                        exact_command = true;
                    }
                }
            }
        }
        let raw_command = candidates.first().copied().unwrap_or(&event.subject);
        let (command, unchanged) = protected_text(raw_command);
        let command_complete = exact_command
            && unchanged
            && candidates.len() == 1
            && inputs_valid
            && commands_valid
            && !flag(evidence, "command_clipped")
            && !flag(evidence, "input_clipped")
            && event.detail_clipped != Some(true);
        let (exits, exits_valid) = entries(evidence.get("exits"), 2);
        let mut exit_codes = BTreeSet::new();
        let mut malformed = false;
        for row in exits {
            if row[1].is_null() {
                continue;
            }
            match exit_code(&row[1]) {
                Some(value) => {
                    exit_codes.insert(value);
                }
                None => malformed = true,
            }
        }
        let exit_code = if exit_codes.len() == 1 {
            exit_codes.first().copied()
        } else {
            None
        };
        let reported = event
            .reported_result
            .as_deref()
            .unwrap_or(&event.result)
            .to_ascii_lowercase();
        let failed = failure(&reported);
        let passed = matches!(reported.as_str(), "ok" | "success" | "passed" | "completed");
        let is_error = flag(evidence, "is_error");
        let conflict = !exits_valid
            || malformed
            || flag(evidence, "terminal_incomplete")
            || exit_codes.len() > 1
            || (exit_code == Some(0) && (failed || is_error))
            || (exit_code.is_some_and(|code| code != 0) && passed);
        let result = if conflict {
            "unknown"
        } else if is_error || failed || exit_code.is_some_and(|code| code != 0) {
            "failed"
        } else if passed || exit_code == Some(0) {
            "passed"
        } else {
            "unknown"
        };
        let basis = if malformed {
            "invalid recorded exit code"
        } else if conflict {
            "conflicting recorded results"
        } else if exit_code.is_some() {
            "recorded exit code"
        } else if result == "unknown" {
            "no terminal result"
        } else {
            "producer result label"
        };
        let informational = command_complete
            && !conflict
            && !is_error
            && exit_code == Some(1)
            && matches!(
                reported.as_str(),
                "unknown" | "informational" | "error" | "failed" | "failure" | ""
            )
            && expected_nonzero(&command);
        let (outputs, outputs_valid) = entries(evidence.get("outputs"), 3);
        let mut output_kind = None;
        let mut output_origin = None;
        let mut output = String::new();
        let mut output_clipped = flag(evidence, "output_clipped")
            || !outputs_valid
            || event.detail_clipped == Some(true);
        for kind in ["stderr", "error", "output", "stdout"] {
            if let Some(row) = outputs
                .iter()
                .find(|row| row[1].as_str() == Some(kind) && !row[2].is_null())
            {
                let rendered = protected_value(&row[2]);
                (output, output_clipped) = (rendered.0, output_clipped || !rendered.1);
                output_kind = Some(kind);
                output_origin = row[0].as_str().map(str::to_owned);
                break;
            }
        }
        // For older receivers only an explicit detail field of an occupied
        // failing operation is output. It never supplies input or exit status.
        if qualified.is_none()
            && event.detail_role.as_deref() != Some("operation_v1")
            && matches!(
                event.domain.as_str(),
                "shell" | "file" | "tool" | "mcp" | "browser"
            )
            && let Some(Value::String(text)) = &event.detail
        {
            let rendered = protected_text(text);
            output = rendered.0;
            output_clipped |= !rendered.1;
            output_kind = Some("output");
            output_origin = Some("detail".into());
        }
        let cwd = evidence["cwd"]
            .as_str()
            .filter(|cwd| !cwd.trim().is_empty());
        let tool = evidence["tool"]
            .as_str()
            .filter(|tool| !tool.trim().is_empty());
        let fingerprint = if command_complete
            && event.domain == "shell"
            && inputs.len() <= 1
            && cwd.is_some()
            && tool.is_some()
        {
            let exact = json!({
                "version": 1,
                "input": { "command": command, "cwd": cwd, "tool": tool, "server": evidence["server"],
                    "arguments": inputs.first().map(|row| &row[1]), "domain": event.domain, "action": event.action }
            });
            let (rendered, complete) = protected_value(&exact);
            complete.then(|| format!("{:x}", Sha256::digest(rendered.as_bytes())))
        } else {
            None
        };
        Self {
            command,
            command_complete,
            result,
            exit_code,
            conflict,
            basis,
            output,
            output_origin,
            output_kind,
            output_clipped,
            informational,
            fingerprint,
        }
    }
}

fn flag(evidence: &Value, key: &str) -> bool {
    evidence[key].as_bool() == Some(true)
}

fn entries(value: Option<&Value>, width: usize) -> (Vec<&[Value]>, bool) {
    let Some(value) = value else {
        return (Vec::new(), true);
    };
    let Some(rows) = value.as_array().filter(|rows| rows.len() <= 8) else {
        return (Vec::new(), false);
    };
    let result: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let row = row.as_array()?.as_slice();
            let origin = row.first()?.as_str()?;
            if row.len() != width
                || origin.is_empty()
                || origin.len() > 128
                || !origin
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.: -".contains(&b))
            {
                return None;
            }
            Some(row)
        })
        .collect();
    let valid = result.len() == rows.len();
    (result, valid)
}

pub(super) fn failure(result: &str) -> bool {
    matches!(
        result,
        "error" | "failed" | "failure" | "denied" | "blocked" | "policy_denied" | "incomplete"
    )
}

fn exit_code(value: &Value) -> Option<i32> {
    if let Some(number) = value.as_i64() {
        return i32::try_from(number).ok();
    }
    let text = value.as_str()?;
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn credential_key(key: &str) -> bool {
    let label: String = key
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    [
        "authorization",
        "apikey",
        "password",
        "privatekey",
        "credential",
        "bearer",
        "accesstoken",
        "refreshtoken",
        "secret",
        "awsaccesskey",
        "authtoken",
        "sessiontoken",
    ]
    .iter()
    .any(|fragment| label.contains(fragment))
        || label == "token"
}

fn protected_text(text: &str) -> (String, bool) {
    if text.len() > 32_768 {
        return ("[Text omitted: projection limit]".into(), false);
    }
    let safe = safe_text(text);
    let labels = safe.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')));
    let credential = labels.into_iter().any(credential_key)
        || ["sk-", "ghp_", "github_pat_", "xoxb-", "AKIA"]
            .iter()
            .any(|prefix| safe.contains(prefix));
    if credential {
        return ("[REDACTED credential-bearing text]".into(), false);
    }
    let unchanged = safe == text && !safe.to_ascii_lowercase().contains("[redact");
    (safe, unchanged)
}

fn protected_value(value: &Value) -> (String, bool) {
    fn visit(value: &Value, depth: usize, nodes: &mut usize, complete: &mut bool) -> Value {
        if depth > 12 || *nodes == 0 {
            *complete = false;
            return json!("[Payload omitted]");
        }
        *nodes -= 1;
        match value {
            Value::String(text) => {
                let (text, unchanged) = protected_text(text);
                *complete &= unchanged;
                Value::String(text)
            }
            Value::Array(rows) => {
                *complete &= rows.len() <= 128;
                Value::Array(
                    rows.iter()
                        .take(128)
                        .map(|row| visit(row, depth + 1, nodes, complete))
                        .collect(),
                )
            }
            Value::Object(rows) => {
                let mut result = Map::new();
                for (key, child) in rows.iter().take(128) {
                    let safe = safe_text(key);
                    if credential_key(key) || safe != *key {
                        *complete = false;
                        result.insert(safe, json!("[REDACTED]"));
                    } else {
                        result.insert(safe, visit(child, depth + 1, nodes, complete));
                    }
                }
                *complete &= rows.len() <= 128;
                Value::Object(result)
            }
            value => value.clone(),
        }
    }
    let mut nodes = 512;
    let mut complete = true;
    let safe = visit(value, 0, &mut nodes, &mut complete);
    let output = match safe {
        Value::String(text) => text,
        value => value.to_string(),
    };
    let (clipped, omitted) = clip(&output, 16_384);
    (clipped.to_owned(), complete && !omitted)
}

pub(super) struct Analysis {
    pub command: String,
    pub kind: String,
    pub compound: bool,
}

pub(super) fn analyze(command: &str) -> Option<Analysis> {
    let command = unwrap_shell(command)?;
    let groups = groups(&command)?;
    let mut kinds = Vec::new();
    for words in &groups {
        let action = kind(words);
        if let Some(kind) = action {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        } else if !(groups.len() > 1 && words.len() == 2 && words[0] == "cd") {
            return None;
        }
    }
    if kinds.is_empty() {
        return None;
    }
    Some(Analysis {
        command,
        kind: kinds.join(" + "),
        compound: groups.len() > 1,
    })
}

fn unwrap_shell(command: &str) -> Option<String> {
    if command.len() > MAX_COMMAND || command.contains(['\n', '\r', '\0']) {
        return None;
    }
    let mut command = command.to_owned();
    for _ in 0..4 {
        let words = groups(&command)?;
        let first = words.first()?.first()?.rsplit('/').next()?;
        if !matches!(first, "sh" | "bash" | "zsh" | "dash" | "ksh") {
            return Some(command);
        }
        if words.len() != 1 || words[0].len() < 3 {
            return None;
        }
        let words = &words[0];
        let flags = &words[1..words.len() - 1];
        if !matches!(flags.last()?.as_str(), "-c" | "-lc" | "-cl")
            || flags[..flags.len() - 1]
                .iter()
                .any(|flag| !matches!(flag.as_str(), "-l" | "--login" | "--noprofile" | "--norc"))
        {
            return None;
        }
        command = words.last()?.clone();
    }
    None
}

// Parse literal shell words only. The result is used for display
// classification. No attempt is made to expand or execute captured text.
fn groups(command: &str) -> Option<Vec<Vec<String>>> {
    if command.contains(['`', '$', '#', '\n', '\r', '|', ';', '<', '>']) {
        return None;
    }
    let mut groups = vec![Vec::new()];
    let mut word = String::new();
    let mut has_word = false;
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else if ch == '\\' && active == '"' && matches!(chars.peek(), Some('"' | '\\')) {
                word.push(chars.next()?);
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            has_word = true;
            quote = Some(ch);
        } else if ch == '\\' {
            has_word = true;
            word.push(chars.next()?);
        } else if ch.is_ascii_whitespace() || ch == '&' {
            if has_word {
                groups.last_mut()?.push(std::mem::take(&mut word));
                has_word = false;
            }
            if ch == '&' {
                if chars.next() != Some('&') || groups.last()?.is_empty() {
                    return None;
                }
                groups.push(Vec::new());
            }
        } else {
            if matches!(ch, '(' | ')' | '{' | '}' | '*' | '?' | '~') {
                return None;
            }
            has_word = true;
            word.push(ch);
        }
    }
    if quote.is_some() {
        return None;
    }
    if has_word {
        groups.last_mut()?.push(word);
    }
    (!groups.last()?.is_empty()).then_some(groups)
}

fn kind(mut words: &[String]) -> Option<String> {
    const INFO: &[&str] = &[
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
    ];
    while let Some((name, value)) = words.first()?.split_once('=') {
        if name.is_empty()
            || !name.bytes().enumerate().all(|(i, byte)| {
                byte.is_ascii_alphabetic() || byte == b'_' || (i > 0 && byte.is_ascii_digit())
            })
        {
            break;
        }
        if name.eq_ignore_ascii_case("npm_config_if_present")
            || (name.eq_ignore_ascii_case("pytest_addopts")
                && value
                    .split_whitespace()
                    .any(|word| INFO.contains(&word.split('=').next().unwrap_or(word))))
        {
            return None;
        }
        words = &words[1..];
    }
    if words
        .iter()
        .any(|word| INFO.contains(&word.split('=').next().unwrap_or(word)))
    {
        return None;
    }
    if matches!(words.first()?.as_str(), "uv" | "poetry")
        && words.get(1).is_some_and(|word| word == "run")
    {
        words = &words[2..];
        while words.first().is_some_and(|word| {
            matches!(
                word.as_str(),
                "--no-sync" | "--offline" | "--frozen" | "--locked" | "--no-project" | "--active"
            )
        }) {
            words = &words[1..];
        }
    }
    let executable = words.first()?.rsplit('/').next()?;
    let arg = words.get(1).map(String::as_str).unwrap_or_default();
    let present = |value: &str| words[1..].iter().any(|word| word == value);
    let python = executable.strip_prefix("python").is_some_and(|suffix| {
        suffix.is_empty()
            || suffix
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    });
    let result = match executable {
        "pytest" | "mypy" | "pyright" | "flake8" | "pylint" => executable.to_owned(),
        "jest" | "vitest" if arg != "list" => executable.to_owned(),
        "tsc" | "eslint" if !present("-v") => executable.to_owned(),
        "ruff" if arg == "check" || (arg == "format" && present("--check")) => "ruff".into(),
        "cargo" if matches!(arg, "test" | "check" | "clippy") => format!("cargo {arg}"),
        "go" if matches!(arg, "test" | "vet")
            && !present("-n")
            && !words
                .iter()
                .any(|word| word == "-list" || word.starts_with("-list=")) =>
        {
            format!("go {arg}")
        }
        "make" if matches!(arg, "test" | "check" | "lint") && !present("-n") => {
            format!("make {arg}")
        }
        "git" if arg == "diff" && present("--check") => "git diff --check".into(),
        "npm" | "pnpm" | "yarn" | "bun" => {
            let action = if arg == "run" {
                words.get(2)?.as_str()
            } else {
                arg
            };
            let (head, suffix) = action
                .split_once(':')
                .map_or((action, None), |(head, tail)| (head, Some(tail)));
            if !matches!(head, "test" | "check" | "lint" | "typecheck" | "type-check")
                || suffix.is_some_and(|tail| {
                    tail.is_empty()
                        || !tail
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b":_-".contains(&b))
                })
            {
                return None;
            }
            action.into()
        }
        _ if python
            && arg == "-m"
            && words
                .get(2)
                .is_some_and(|module| matches!(module.as_str(), "unittest" | "pytest")) =>
        {
            words[2].clone()
        }
        _ => return None,
    };
    Some(result)
}

fn expected_nonzero(command: &str) -> bool {
    let Some(command) = unwrap_shell(command) else {
        return false;
    };
    let Some(groups) = groups(&command) else {
        return false;
    };
    if groups.len() != 1 || groups[0].len() < 2 || groups[0].iter().any(|word| word == "--check") {
        return false;
    }
    let words = &groups[0];
    match words[0].rsplit('/').next() {
        Some("grep" | "rg" | "diff") => true,
        Some("git") => {
            words[1] == "diff"
                && words
                    .iter()
                    .any(|word| word == "--quiet" || word == "--exit-code")
        }
        _ => false,
    }
}
