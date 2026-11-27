//! Content-free translation of one Codex rollout JSONL row.
//! Completion and interruption require explicit source events. A PR, tool
//! result, EOF or ordinary assistant message never supplies a terminal state.

mod approval;

use agenturm_jsonl_source::Result;
use agenturm_protocol::observation::{
    FileEdit, McpCall, McpMethod, RelationshipKind, is_github_pull_request, is_mcp_identifier,
    is_valid_mcp_metadata,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::path::Path;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    pub worker_id: String,
    pub turn_id: String,
    parent_thread: Option<String>,
    guardian: Option<approval::Guardian>,
    header_bytes: usize,
    in_copied_context: bool,
    header_discovered: bool,
}

impl Context {
    fn copied_checkpoint(&self) -> String {
        // Checkpoints may be longer than native IDs. Keep this marker outside
        // the native turn ID length limit to distinguish older checkpoints.
        format!("fork-copy:{:0>200}", self.session_id)
    }

    fn discovery_checkpoint(&self) -> String {
        // Longer than every native turn ID and distinct from copied context.
        format!("session-discovered:{:0>200}", self.session_id)
    }

    pub(crate) fn checkpoint(&self) -> String {
        if self.in_copied_context {
            self.copied_checkpoint()
        } else if self.turn_id.is_empty() && self.header_discovered {
            self.discovery_checkpoint()
        } else {
            self.turn_id.clone()
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: &str) {
        self.header_discovered = checkpoint == self.discovery_checkpoint();
        self.in_copied_context =
            self.parent_thread.is_some() && checkpoint == self.copied_checkpoint();
        self.turn_id = if self.in_copied_context || self.header_discovered {
            String::new()
        } else {
            checkpoint.to_owned()
        };
    }

    pub(crate) fn omit_discovery(&mut self) {
        self.header_discovered = false;
    }
}

pub(crate) fn header_identity(raw: &[u8]) -> Result<Context> {
    let row: Value = serde_json::from_slice(raw)?;
    if row.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err(invalid("Codex source must start with session_meta"));
    }
    let payload = row
        .get("payload")
        .and_then(Value::as_object)
        .ok_or("Codex header payload missing")?;
    let thread = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Codex thread ID missing")?;
    let root = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or(thread);
    if !identifier(thread) || !identifier(root) {
        return Err(invalid("invalid Codex thread identity"));
    }
    Ok(Context {
        session_id: thread.to_owned(),
        worker_id: if root != thread {
            thread.to_owned()
        } else {
            String::new()
        },
        turn_id: String::new(),
        parent_thread: payload
            .get("source")
            .and_then(|source| source.pointer("/subagent/thread_spawn/parent_thread_id"))
            .and_then(Value::as_str)
            .filter(|parent| identifier(parent) && *parent != thread)
            .map(str::to_owned),
        guardian: approval::Guardian::from_header(raw, thread)?,
        header_bytes: raw.len(),
        in_copied_context: false,
        header_discovered: false,
    })
}

fn identity(context: &Context, operation: Option<&str>) -> Value {
    let mut value = json!({"session_id":context.session_id});
    if !context.worker_id.is_empty() {
        value["worker_id"] = json!(context.worker_id);
    }
    if !context.turn_id.is_empty() {
        value["turn_id"] = json!(context.turn_id);
    }
    if let Some(operation) = operation {
        value["operation_id"] = json!(operation);
    }
    value
}

fn fact(
    id: String,
    kind: &str,
    stamp: &str,
    context: &Context,
    payload: Value,
    operation: Option<&str>,
) -> Value {
    json!({"event_id":id,"revision":0,"kind":kind,"time_ns":stamp,
           "identity":identity(context, operation),"payload":payload})
}

fn diagnostic(id: String, stamp: &str, code: &str, severity: &str) -> Value {
    json!({"event_id":id,"revision":0,"kind":"diagnostic","time_ns":stamp,
           "payload":{"code":code,"severity":severity}})
}

fn stamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Codex row timestamp missing")?;
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)?;
    let nanos = parsed.unix_timestamp_nanos();
    if nanos < 0 || nanos > i64::MAX as i128 {
        return Err(invalid("Codex timestamp outside supported range"));
    }
    Ok(nanos.to_string())
}

fn discovered(context: &mut Context, origin: &str, time_ns: &str) -> Value {
    context.header_discovered = true;
    fact(
        format!("codex:{origin}:0:discovered"),
        "session_discovered",
        time_ns,
        context,
        json!({}),
        None,
    )
}

pub(crate) fn resume_verified_header(
    raw: &[u8],
    context: &mut Context,
    origin: &str,
) -> Result<Option<Value>> {
    if context.guardian.is_some()
        || context.header_discovered
        || context.in_copied_context
        || !context.turn_id.is_empty()
    {
        return Ok(None);
    }
    let row: Value = serde_json::from_slice(raw)?;
    let payload = &row["payload"];
    // Older readers already admitted this header's relationship as a
    // session-scoped fact. Those sessions already appear on the roster.
    if header_relationship(payload, &context.session_id).is_some() {
        return Ok(None);
    }
    Ok(stamp(&row)
        .or_else(|_| stamp(payload))
        .ok()
        .map(|time_ns| discovered(context, origin, &time_ns)))
}

fn header_relationship(payload: &Value, child: &str) -> Option<Value> {
    let direct = payload
        .get("parent_thread_id")
        .filter(|value| !value.is_null());
    let nested = payload
        .pointer("/source/subagent/thread_spawn/parent_thread_id")
        .filter(|value| !value.is_null());
    if let (Some(direct), Some(nested)) = (direct, nested)
        && direct != nested
    {
        return None;
    }
    let (kind, parent) = match direct.or(nested) {
        Some(parent) => (RelationshipKind::Delegated, parent),
        None => (RelationshipKind::Forked, payload.get("forked_from_id")?),
    };
    let parent = parent.as_str()?;
    (identifier(parent) && parent != child)
        .then(|| json!({"relationship_kind":kind,"parent_session_id":parent}))
}

fn turn_from(payload: &Value, context: &mut Context, clear_if_missing: bool) -> Result<()> {
    if let Some(raw) = payload.get("turn_id") {
        let turn = raw.as_str().ok_or("invalid Codex turn ID")?;
        if !identifier(turn) {
            return Err(invalid("invalid Codex turn ID"));
        }
        context.turn_id = turn.to_owned();
    } else if clear_if_missing {
        context.turn_id.clear();
    }
    Ok(())
}

fn historical_turn(payload: &Value, current: &Context) -> bool {
    payload
        .get("turn_id")
        .and_then(Value::as_str)
        .is_some_and(|turn| !current.turn_id.is_empty() && turn != current.turn_id)
}

fn opaque_operation(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.len() > 4096 {
        return Err(invalid("invalid Codex operation ID"));
    }
    Ok(format!("{:x}", Sha256::digest(raw.as_bytes())))
}

fn tool_category(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("mcp__") {
        "mcp"
    } else if matches!(
        lower.as_str(),
        // `exec` is the current Codex custom_tool_call name for command
        // execution in actual rollouts; it must stay shell-categorized or
        // command-detail reveals reject it.
        "exec" | "exec_command" | "shell_command" | "bash" | "sh" | "terminal" | "functions.exec"
    ) {
        "shell"
    } else if matches!(
        lower.as_str(),
        "read_file" | "write_file" | "apply_patch" | "edit" | "read" | "write"
    ) {
        "file"
    } else {
        "other"
    }
}

fn item_category(kind: &str) -> Option<&'static str> {
    match kind {
        "CommandExecution" => Some("shell"),
        "FileChange" => Some("file"),
        "McpToolCall" => Some("mcp"),
        "Extension" => Some("other"),
        _ => None,
    }
}

fn explicit_tool_result(item: &Value) -> Result<Option<&'static str>> {
    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    let exit = match item.get("exit_code") {
        Some(value) => Some(value.as_i64().ok_or("invalid Codex exit code")?),
        None => None,
    };
    if status.eq_ignore_ascii_case("denied") || status.eq_ignore_ascii_case("declined") {
        return Ok(Some("denied"));
    }
    if matches!(status, "failed" | "error") || exit.is_some_and(|code| code != 0) {
        return Ok(Some("failed"));
    }
    if let Some(error) = item.get("result").and_then(|value| value.get("isError")) {
        return match error.as_bool() {
            Some(true) => Ok(Some("failed")),
            Some(false) => Ok(Some("succeeded")),
            None => Err(invalid("invalid Codex result flag")),
        };
    }
    if matches!(status, "completed" | "success" | "ok") || exit == Some(0) {
        return Ok(Some("succeeded"));
    }
    Ok(None)
}

fn mcp_duration_ns(item: &Value) -> Option<u64> {
    let duration = item.get("duration")?.as_object()?;
    let seconds = duration.get("secs")?.as_u64()?;
    let nanos = duration.get("nanos")?.as_u64()?;
    if nanos >= 1_000_000_000 {
        return None;
    }
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(nanos)
        .filter(|duration| *duration <= i64::MAX as u64)
}

fn mcp_call(item: &Value, result: &str) -> Option<McpCall> {
    let label = |field| {
        item.get(field)
            .and_then(Value::as_str)
            .filter(|name| is_mcp_identifier(name))
    };
    let server = label("server");
    let tool = label("tool");
    let duration_ns = mcp_duration_ns(item);
    is_valid_mcp_metadata(server, tool, duration_ns, result).then(|| McpCall {
        server: server.map(str::to_owned),
        tool: tool.map(str::to_owned),
        method: McpMethod::ToolsCall,
        duration_ns,
        result: result.to_owned(),
    })
}

fn file_edit(item: &Value) -> Option<FileEdit> {
    let changes = item.get("changes")?.as_object()?;
    if !(1..=FileEdit::MAX_CHANGES).contains(&changes.len()) {
        return None;
    }
    let mut summary = FileEdit::default();
    for change in changes.values() {
        match change.get("type")?.as_str()? {
            "add" => summary.added += 1,
            "delete" => summary.deleted += 1,
            "update" => match change.get("move_path") {
                None | Some(Value::Null) => summary.updated += 1,
                Some(Value::String(path)) if !path.is_empty() => summary.moved += 1,
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(summary)
}

fn check_category(item: &Value) -> Option<&'static str> {
    let command = item.get("command")?.as_array()?;
    if command.is_empty() || command.len() > 128 {
        return None;
    }
    let words: Vec<&str> = command.iter().map(Value::as_str).collect::<Option<_>>()?;
    let expanded;
    let words = match words.as_slice() {
        [shell, "-c" | "-lc", raw]
            if matches!(executable_name(shell), Some("bash" | "dash" | "sh" | "zsh")) =>
        {
            expanded = single_shell_command(raw)?;
            expanded.iter().map(String::as_str).collect()
        }
        _ => words,
    };
    let (program, args) = words.split_first()?;
    let (program, args) = match (executable_name(program)?, args) {
        ("uv", ["run", args @ ..]) => {
            let args = args.strip_prefix(&["--no-sync"]).unwrap_or(args);
            let (program, args) = args.split_first()?;
            (executable_name(program)?, args)
        }
        (program, args) => (program, args),
    };
    if args.iter().any(|arg| {
        let option = arg.split_once('=').map_or(*arg, |(option, _)| option);
        matches!(
            option,
            "-h" | "--help"
                | "-V"
                | "--version"
                | "--collect-only"
                | "--co"
                | "--fixtures"
                | "--fixtures-per-test"
                | "--markers"
                | "--trace-config"
                | "--dry-run"
                | "--list"
                | "--listTests"
                | "--listFilesOnly"
                | "--showConfig"
                | "--show-files"
                | "--show-settings"
                | "--if-present"
                | "--no-run"
        ) || (program == "go" && matches!(option, "-list" | "-n"))
    }) {
        return None;
    }
    match (program, args) {
        ("cargo" | "go", ["test", ..])
        | ("pytest", _)
        | ("python" | "python3", ["-m", "pytest" | "unittest", ..])
        | ("npm" | "pnpm", ["test", ..]) => Some("test"),
        ("cargo", ["clippy", ..]) | ("ruff", ["check", ..]) => Some("lint"),
        ("cargo", ["fmt", flags @ ..]) | ("ruff", ["format", flags @ ..])
            if flags.contains(&"--check") =>
        {
            Some("lint")
        }
        ("git", ["diff", flags @ ..])
            if flags.contains(&"--check")
                && !flags.iter().any(|flag| {
                    matches!(
                        *flag,
                        "--quiet" | "--exit-code" | "--no-patch" | "--name-only" | "--name-status"
                    )
                }) =>
        {
            Some("lint")
        }
        ("cargo" | "go", ["build", ..]) => Some("build"),
        ("npm" | "pnpm", ["run", name]) | ("yarn", [name]) => {
            if *name == "check" || *name == "typecheck" || *name == "lint" {
                Some("lint")
            } else if *name == "build" {
                Some("build")
            } else if *name == "test" || name.starts_with("test:") {
                Some("test")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn check_result(item: &Value) -> Option<&'static str> {
    match (
        item.get("status").and_then(Value::as_str),
        item.get("exit_code").and_then(Value::as_i64),
    ) {
        (Some("completed" | "success" | "ok"), Some(0)) => Some("passed"),
        (Some("failed" | "error"), Some(1..=255)) => Some("failed"),
        _ => None,
    }
}

fn task_completion(payload: &Value) -> Option<(&'static str, &'static str)> {
    match payload.get("error") {
        None | Some(Value::Null) => Some(("completed", "codex_task_complete")),
        Some(error)
            if error
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()) =>
        {
            Some(("failed", "codex_task_failed"))
        }
        _ => None,
    }
}

fn executable_name(path: &str) -> Option<&str> {
    Path::new(path).file_name()?.to_str()
}

fn is_pr_create<'a>(mut words: impl Iterator<Item = &'a str>) -> bool {
    words.next().and_then(executable_name) == Some("gh")
        && words.next() == Some("pr")
        && words.next() == Some("create")
        && !words.any(|word| {
            matches!(word, "--help" | "-h" | "--dry-run") || word.starts_with("--dry-run=")
        })
}

// Split a single shell command only after ruling out syntax that could run
// another command or expand arguments. Quoted title and body values are data.
fn single_shell_command(raw: &str) -> Option<Vec<String>> {
    if raw.len() > 16 * 1024 || raw.as_bytes().contains(&0) {
        return None;
    }
    let mut quote = None;
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        match (quote, character) {
            (Some(open), close) if open == close => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => {
                chars.next()?;
            }
            (_, '$' | '`') => return None,
            (None, '\'' | '"') => quote = Some(character),
            (
                None,
                ';' | '|' | '&' | '>' | '<' | '(' | ')' | '{' | '}' | '*' | '?' | '[' | ']' | '#'
                | '!' | '~' | '\n' | '\r',
            ) => return None,
            _ => {}
        }
    }
    if quote.is_some() {
        return None;
    }
    shlex::split(raw)
}

fn raised_pull_request(item: &Value) -> Option<(&str, u64)> {
    if item.get("exit_code").and_then(Value::as_i64) != Some(0)
        || item.get("status").and_then(Value::as_str) != Some("completed")
    {
        return None;
    }
    let command = item.get("command")?.as_array()?;
    if command.len() > 128 {
        return None;
    }
    let words = command
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    let creates_pr = match words.as_slice() {
        [shell, "-c" | "-lc", raw]
            if matches!(executable_name(shell), Some("bash" | "dash" | "sh" | "zsh")) =>
        {
            let expanded = single_shell_command(raw)?;
            is_pr_create(expanded.iter().map(String::as_str))
        }
        _ => is_pr_create(words.into_iter()),
    };
    if !creates_pr {
        return None;
    }
    let output = item.get("aggregated_output")?.as_str()?;
    if output.len() > 512 {
        return None;
    }
    let (repository, raw_number) = output
        .trim()
        .strip_prefix("https://github.com/")?
        .rsplit_once("/pull/")?;
    if raw_number.starts_with('0') || !raw_number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = raw_number.parse().ok()?;
    is_github_pull_request(repository, number).then_some((repository, number))
}

fn usage(payload: &Value) -> Result<Option<Value>> {
    let Some(raw) = payload.get("usage") else {
        return Ok(None);
    };
    let fields = raw.as_object().ok_or("Codex usage is not an object")?;
    let mut result = serde_json::Map::new();
    for (source, target) in [
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cached_input_tokens", "cache_read_tokens"),
        ("cache_write_input_tokens", "cache_write_tokens"),
        ("total_tokens", "total_tokens"),
    ] {
        if let Some(value) = fields.get(source) {
            let count = value.as_u64().ok_or("invalid Codex usage counter")?;
            if count > i64::MAX as u64 {
                return Err(invalid("Codex usage counter exceeds supported range"));
            }
            result.insert(target.to_owned(), json!(count));
        }
    }
    Ok((!result.is_empty()).then_some(Value::Object(result)))
}

/// Translate one complete row. The shared runtime only checkpoints `next`
/// after every fact from this row fits in a single acknowledged batch.
pub(crate) fn translate_line(
    raw: &[u8],
    context: &Context,
    origin: &str,
    offset: u64,
) -> Result<(Vec<Value>, Context)> {
    let row: Value = serde_json::from_slice(raw)?;
    let kind = row
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Codex row type missing")?;
    let mut next = context.clone();
    if kind == "session_meta" {
        let header = header_identity(raw)?;
        if let Some(guardian) = &context.guardian {
            if header.session_id != context.session_id
                || header.worker_id != context.worker_id
                || header.guardian.as_ref() != Some(guardian)
            {
                return Err(invalid("Codex guardian identity changed inside one file"));
            }
            let facts = if offset == 0 {
                stamp(&row)
                    .or_else(|_| stamp(&row["payload"]))
                    .ok()
                    .map(|time| guardian.classification(context, origin, &time))
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            };
            return Ok((facts, next));
        }
        if header.guardian.is_some() {
            return Err(invalid("Codex guardian header changed inside one file"));
        }
        if header.session_id != context.session_id || header.worker_id != context.worker_id {
            // A forked worker can embed the parent's rollout immediately
            // after its own header. Preserve the worker's original identity.
            if offset != context.header_bytes as u64
                || context.parent_thread.as_deref() != Some(&header.session_id)
            {
                return Err(invalid("Codex session identity changed inside one file"));
            }
            next.in_copied_context = true;
            return Ok((Vec::new(), next));
        }
        let payload = &row["payload"];
        let mut facts = Vec::new();
        if offset == 0
            && let Ok(time_ns) = stamp(&row).or_else(|_| stamp(payload))
        {
            facts.push(discovered(&mut next, origin, &time_ns));
            if let Some(relationship) = header_relationship(payload, &header.session_id) {
                facts.push(fact(
                    format!("codex:{origin}:{offset}:relationship"),
                    "relationship",
                    &time_ns,
                    &header,
                    relationship,
                    None,
                ));
            }
        }
        return Ok((facts, next));
    }
    let base = format!("codex:{origin}:{offset}");
    if let Some(guardian) = &context.guardian {
        return guardian.translate(raw, &row, context, kind, &base);
    }
    if context.in_copied_context {
        let payload = &row["payload"];
        if kind == "event_msg"
            && payload["type"] == "thread_settings_applied"
            && payload["thread_id"].as_str() == Some(context.session_id.as_str())
        {
            next.in_copied_context = false;
            return Ok((
                vec![diagnostic(
                    format!("{base}:inherited"),
                    &stamp(&row)?,
                    "codex_inherited_prefix",
                    "info",
                )],
                next,
            ));
        }
        return Ok((Vec::new(), next));
    }
    let time_ns = stamp(&row)?;
    let payload = row.get("payload").ok_or("Codex row payload missing")?;
    if !payload.is_object() {
        return Err(invalid("Codex row payload is not an object"));
    }
    let mut row_context = context.clone();
    turn_from(payload, &mut row_context, false)?;
    let event = payload.get("type").and_then(Value::as_str).unwrap_or("");
    let mut facts = Vec::new();
    match kind {
        "turn_context" => {
            if payload.get("model").and_then(Value::as_str) == Some("codex-auto-review") {
                return Err(invalid("internal Codex review is outside this adapter"));
            }
        }
        "token_usage_record" => {
            let response = payload
                .get("response_id")
                .and_then(Value::as_str)
                .ok_or("Codex usage response ID missing")?;
            let response_hash = opaque_operation(response)?;
            if let Some(counters) = usage(payload)? {
                facts.push(fact(
                    format!("codex:{origin}:usage:{response_hash}"),
                    "usage",
                    &time_ns,
                    &row_context,
                    counters,
                    None,
                ));
            } else {
                facts.push(diagnostic(
                    format!("{base}:usage-unavailable"),
                    &time_ns,
                    "codex_usage_unavailable",
                    "warning",
                ));
            }
        }
        "response_item" => match event {
            "message" => {
                let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                if matches!(role, "user" | "assistant") {
                    if role == "user" {
                        // Do not attach an unmarked new user message to the
                        // previous turn's exact drilldown key.
                        if kind == "response_item" && payload.get("turn_id").is_none() {
                            next.turn_id = format!("codex-user:{offset}");
                        } else {
                            turn_from(payload, &mut next, true)?;
                        }
                        row_context = next.clone();
                    }
                    facts.push(fact(
                        format!("{base}:message"),
                        "message",
                        &time_ns,
                        &row_context,
                        json!({"role":role}),
                        None,
                    ));
                } else {
                    facts.push(diagnostic(
                        format!("{base}:role"),
                        &time_ns,
                        "unsupported_codex_role",
                        "info",
                    ));
                }
            }
            "function_call" | "custom_tool_call" => {
                let call = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .ok_or("Codex tool call ID missing")?;
                let operation = opaque_operation(call)?;
                let category =
                    tool_category(payload.get("name").and_then(Value::as_str).unwrap_or(""));
                facts.push(fact(
                    format!("{base}:tool-call"),
                    "tool",
                    &time_ns,
                    &row_context,
                    json!({"category":category,"result":"started"}),
                    Some(&operation),
                ));
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .ok_or("Codex tool result ID missing")?;
                let operation = opaque_operation(call)?;
                if let Some(flag) = payload.get("isError") {
                    let failed = flag.as_bool().ok_or("invalid Codex result flag")?;
                    facts.push(fact(format!("{base}:tool-result"), "tool", &time_ns,
                        &row_context, json!({"category":"other","result":if failed {"failed"} else {"succeeded"}}),
                        Some(&operation)));
                } else {
                    facts.push(diagnostic(
                        format!("{base}:tool-result-unknown"),
                        &time_ns,
                        "codex_tool_result_unknown",
                        "info",
                    ));
                }
            }
            _ => facts.push(diagnostic(
                format!("{base}:response"),
                &time_ns,
                "unsupported_codex_response",
                "info",
            )),
        },
        "event_msg" => {
            match event {
                "task_started" => {
                    turn_from(payload, &mut next, true)?;
                    row_context = next.clone();
                    facts.push(fact(
                        format!("{base}:lifecycle"),
                        "lifecycle",
                        &time_ns,
                        &row_context,
                        json!({"state":"running"}),
                        None,
                    ));
                }
                "task_complete" | "task_aborted" | "turn_aborted" => {
                    if historical_turn(payload, context) {
                        facts.push(diagnostic(
                            format!("{base}:historical-outcome"),
                            &time_ns,
                            "codex_historical_outcome",
                            "info",
                        ));
                    } else {
                        let terminal = match event {
                            "task_complete" => task_completion(payload),
                            "task_aborted" => Some(("interrupted", "codex_task_aborted")),
                            _ => Some(("interrupted", "codex_turn_aborted")),
                        };
                        if let Some((state, reason)) = terminal {
                            facts.push(fact(
                                format!("{base}:lifecycle"),
                                "lifecycle",
                                &time_ns,
                                &row_context,
                                json!({"state":state,"reason_code":reason}),
                                None,
                            ));
                        } else {
                            facts.push(diagnostic(
                                format!("{base}:unknown-outcome"),
                                &time_ns,
                                "unsupported_codex_event",
                                "warning",
                            ));
                        }
                    }
                }
                "approval_request" | "request_user_input" => {
                    if let Some(request) = approval::request(raw, &row_context, &base, &time_ns) {
                        facts.push(request);
                    }
                    if historical_turn(payload, context) {
                        facts.push(diagnostic(
                            format!("{base}:historical-wait"),
                            &time_ns,
                            "codex_historical_wait",
                            "info",
                        ));
                    } else {
                        facts.push(fact(format!("{base}:lifecycle"), "lifecycle", &time_ns,
                        &row_context, json!({"state":"paused","reason_code":if event == "approval_request" {"codex_approval_request"} else {"codex_user_input_request"}}), None));
                    }
                }
                "item_completed" => {
                    let item = payload.get("item").ok_or("Codex completed item missing")?;
                    let item_kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or("Codex item type missing")?;
                    match item_kind {
                        "UserMessage" | "AgentMessage" => {
                            let role = if item_kind == "UserMessage" {
                                "user"
                            } else {
                                "assistant"
                            };
                            if role == "user" {
                                turn_from(payload, &mut next, true)?;
                                row_context = next.clone();
                            }
                            facts.push(fact(
                                format!("{base}:message"),
                                "message",
                                &time_ns,
                                &row_context,
                                json!({"role":role}),
                                None,
                            ));
                            if role == "user" {
                                facts.push(fact(
                                    format!("{base}:lifecycle"),
                                    "lifecycle",
                                    &time_ns,
                                    &row_context,
                                    json!({"state":"running"}),
                                    None,
                                ));
                            } else if item.get("phase").and_then(Value::as_str)
                                == Some("final_answer")
                            {
                                if historical_turn(payload, context) {
                                    facts.push(diagnostic(
                                        format!("{base}:historical-final"),
                                        &time_ns,
                                        "codex_historical_outcome",
                                        "info",
                                    ));
                                } else {
                                    facts.push(fact(format!("{base}:lifecycle"), "lifecycle", &time_ns,
                                    &row_context, json!({"state":"completed","reason_code":"codex_final_answer"}), None));
                                }
                            }
                        }
                        "CommandExecution" | "FileChange" | "McpToolCall" | "Extension" => {
                            let category =
                                item_category(item_kind).ok_or("unsupported Codex tool")?;
                            if let Some(result) = explicit_tool_result(item)? {
                                let operation = item
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map(opaque_operation)
                                    .transpose()?;
                                facts.push(fact(
                                    format!("{base}:tool"),
                                    "tool",
                                    &time_ns,
                                    &row_context,
                                    json!({"category":category,"result":result}),
                                    operation.as_deref(),
                                ));
                                if item_kind == "McpToolCall"
                                    && let Some(operation) = operation.as_deref()
                                    && let Some(call) = mcp_call(item, result)
                                {
                                    facts.push(fact(
                                        format!("{base}:mcp-call"),
                                        "mcp_call",
                                        &time_ns,
                                        &row_context,
                                        json!(call),
                                        Some(operation),
                                    ));
                                }
                                if item_kind == "FileChange"
                                    && result == "succeeded"
                                    && !row_context.turn_id.is_empty()
                                    && let Some(operation) = operation.as_deref()
                                    && let Some(edit) = file_edit(item)
                                {
                                    facts.push(fact(
                                        format!("{base}:file-edit"),
                                        "file_edit",
                                        &time_ns,
                                        &row_context,
                                        json!(edit),
                                        Some(operation),
                                    ));
                                }
                                if item_kind == "CommandExecution"
                                    && let Some(check) = check_category(item)
                                    && let Some(outcome) = check_result(item)
                                    && let Some(operation) = operation.as_deref()
                                {
                                    facts.push(fact(
                                        format!("{base}:check"),
                                        "check",
                                        &time_ns,
                                        &row_context,
                                        json!({"category":check,"result":outcome}),
                                        Some(operation),
                                    ));
                                }
                                if item_kind == "CommandExecution"
                                    && result == "succeeded"
                                    && payload.get("turn_id").is_some()
                                    && let Some(operation) = operation.as_deref()
                                    && let Some((repository, number)) = raised_pull_request(item)
                                {
                                    facts.push(fact(
                                        format!("{base}:pull-request-raised"),
                                        "pull_request_raised",
                                        &time_ns,
                                        &row_context,
                                        json!({"repository":repository,"number":number}),
                                        Some(operation),
                                    ));
                                }
                            } else {
                                facts.push(diagnostic(
                                    format!("{base}:tool-unknown"),
                                    &time_ns,
                                    "codex_tool_result_unknown",
                                    "info",
                                ));
                            }
                        }
                        "Reasoning" | "ContextCompaction" => {}
                        _ => facts.push(diagnostic(
                            format!("{base}:item"),
                            &time_ns,
                            "unsupported_codex_item",
                            "info",
                        )),
                    }
                }
                "token_count" => facts.push(diagnostic(
                    format!("{base}:legacy-usage"),
                    &time_ns,
                    "codex_legacy_usage_unmapped",
                    "info",
                )),
                _ => facts.push(diagnostic(
                    format!("{base}:event"),
                    &time_ns,
                    "unsupported_codex_event",
                    "info",
                )),
            }
        }
        _ => facts.push(diagnostic(
            format!("{base}:row"),
            &time_ns,
            "unsupported_codex_row",
            "info",
        )),
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &str, payload: Value) -> Vec<u8> {
        json!({"type":kind,"timestamp":"2026-11-14T10:00:00Z","payload":payload})
            .to_string()
            .into_bytes()
    }

    fn translate(payload: Value, context: &Context) -> (Vec<Value>, Context) {
        translate_line(&row("event_msg", payload), context, "generation", 10).unwrap()
    }

    #[test]
    fn current_native_exec_tool_calls_stay_shell_categorized() {
        // Actual 2026-11 rollouts record command execution as custom_tool_call
        // rows with tool name `exec`; command-detail reveals require the
        // shell category.
        for name in ["exec", "EXEC", "exec_command", "functions.exec"] {
            assert_eq!(tool_category(name), "shell", "{name} must be shell");
        }
        assert_eq!(tool_category("executioner"), "other");
        assert_eq!(tool_category("read_file"), "file");
        assert_eq!(tool_category("mcp__x"), "mcp");
    }

    #[test]
    fn native_mcp_metadata_keeps_explicit_names_and_duration_without_content() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let payload = json!({
            "type":"item_completed","turn_id":"turn-1",
            "item":{
                "type":"McpToolCall","id":"mcp-1","server":"github",
                "tool":"search_code","duration":{"secs":2,"nanos":123_456},
                "status":"completed",
                "arguments":{"query":"private invocation"},
                "result":{"isError":true,"content":"private result"},
                "trace_id":"diagnostic-fixture-unused"
            }
        });
        let (facts, _) = translate(payload.clone(), &context);
        assert_eq!(facts.len(), 2);
        let call = &facts[1];
        assert_eq!(call["kind"], "mcp_call");
        assert_eq!(call["identity"], facts[0]["identity"]);
        assert_eq!(call["identity"]["turn_id"], "turn-1");
        assert_eq!(
            call["payload"],
            json!({
                "server":"github","tool":"search_code","method":"tools/call",
                "duration_ns":2_000_123_456u64,"result":"failed"
            })
        );
        assert!(!facts.iter().any(|row| row.to_string().contains("private")));
        assert!(!call.to_string().contains("trace_id"));

        let mut malformed = payload;
        malformed["item"]["server"] = json!("../private-path");
        malformed["item"]["tool"] = json!("raw argument text");
        malformed["item"]["duration"]["nanos"] = json!(1_000_000_000);
        let (facts, _) = translate(malformed, &context);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["kind"], "tool");
    }

    #[test]
    fn session_header_records_explicit_delegation_and_fork() {
        for (payload, relationship_kind) in [
            (
                json!({"id":"child","session_id":"root","parent_thread_id":"parent",
                      "source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}),
                "delegated",
            ),
            (json!({"id":"child","forked_from_id":"parent"}), "forked"),
        ] {
            let raw = row("session_meta", payload);
            let context = header_identity(&raw).unwrap();
            let (facts, _) = translate_line(&raw, &context, "generation", 0).unwrap();
            assert_eq!(facts.len(), 2);
            assert_eq!(facts[0]["identity"]["session_id"], "child");
            assert_eq!(facts[0]["kind"], "session_discovered");
            assert_eq!(facts[1]["kind"], "relationship");
            assert_eq!(
                facts[1]["payload"],
                json!({"relationship_kind":relationship_kind,"parent_session_id":"parent"}),
            );
            assert!(
                translate_line(&raw, &context, "generation", 30)
                    .unwrap()
                    .0
                    .is_empty()
            );
        }

        let nested_only = br#"{"type":"session_meta","payload":{"id":"child","timestamp":"2026-11-14T10:00:00Z","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}"#;
        let context = header_identity(nested_only).unwrap();
        let (facts, _) = translate_line(nested_only, &context, "generation", 0).unwrap();
        assert_eq!(facts[1]["payload"]["relationship_kind"], "delegated");
    }

    #[test]
    fn session_header_omits_missing_conflicting_and_invalid_parent_evidence() {
        for payload in [
            json!({"id":"child","session_id":"root","source":{"subagent":"other"}}),
            json!({"id":"child","parent_thread_id":"parent","source":
                   {"subagent":{"thread_spawn":{"parent_thread_id":"other"}}}}),
            json!({"id":"child","parent_thread_id":"child"}),
            json!({"id":"child","parent_thread_id":"../parent"}),
        ] {
            let raw = row("session_meta", payload);
            let context = header_identity(&raw).unwrap();
            let (facts, _) = translate_line(&raw, &context, "generation", 0).unwrap();
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0]["kind"], "session_discovered");
        }
    }

    #[test]
    fn explicit_terminal_events_keep_distinct_reason_provenance() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let (_, running) = translate(json!({"type":"task_started","turn_id":"turn-1"}), &context);
        for (event, state, reason) in [
            ("task_complete", "completed", "codex_task_complete"),
            ("task_aborted", "interrupted", "codex_task_aborted"),
            ("turn_aborted", "interrupted", "codex_turn_aborted"),
        ] {
            let (facts, _) = translate(
                json!({"type":event,"turn_id":"turn-1","reason":"private reason"}),
                &running,
            );
            assert_eq!(facts[0]["payload"]["state"], state);
            assert_eq!(facts[0]["payload"]["reason_code"], reason);
            assert_eq!(facts[0]["identity"]["turn_id"], "turn-1");
            assert!(!facts[0].to_string().contains("private reason"));
        }
        let (ordinary, _) = translate(
            json!({"type":"item_completed","item":{
            "type":"AgentMessage","phase":"commentary","content":"PR raised"}}),
            &running,
        );
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0]["kind"], "message");
        let (final_answer, _) = translate(
            json!({"type":"item_completed","item":{
            "type":"AgentMessage","phase":"final_answer","content":"private answer"}}),
            &running,
        );
        assert_eq!(
            final_answer[1]["payload"]["reason_code"],
            "codex_final_answer"
        );
        assert!(
            !serde_json::to_string(&final_answer)
                .unwrap()
                .contains("private answer")
        );
    }

    #[test]
    fn task_complete_preserves_a_structured_failure_without_retaining_error_content() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let (_, running) = translate(json!({"type":"task_started","turn_id":"turn-1"}), &context);
        let (facts, _) = translate(
            json!({"type":"task_complete","turn_id":"turn-1","error":{
                "message":"private provider failure","codex_error_info":"other"}}),
            &running,
        );
        assert_eq!(facts[0]["identity"]["turn_id"], "turn-1");
        assert_eq!(
            facts[0]["payload"],
            json!({"state":"failed","reason_code":"codex_task_failed"})
        );
        assert!(
            !serde_json::to_string(&facts)
                .unwrap()
                .contains("private provider failure")
        );
        let (ambiguous, _) = translate(
            json!({"type":"task_complete","turn_id":"turn-1","error":{"unexpected":true}}),
            &running,
        );
        assert_eq!(ambiguous[0]["kind"], "diagnostic");
    }

    #[test]
    fn checks_require_exact_command_shape_and_a_consistent_native_exit() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        for (command, status, exit, expected) in [
            (
                json!([
                    "/usr/bin/zsh",
                    "-lc",
                    ".venv/bin/python -m unittest tests.test_example"
                ]),
                "completed",
                json!(0),
                Some("passed"),
            ),
            (
                json!([
                    "/usr/bin/zsh",
                    "-lc",
                    "uv run --no-sync python -m unittest tests.test_example"
                ]),
                "failed",
                json!(1),
                Some("failed"),
            ),
            (
                json!([
                    "/usr/bin/zsh",
                    "-lc",
                    "python -m unittest tests.test_example"
                ]),
                "completed",
                Value::Null,
                None,
            ),
            (
                json!([
                    "/usr/bin/zsh",
                    "-lc",
                    "python -m unittest tests.test_example"
                ]),
                "failed",
                json!(0),
                None,
            ),
            (
                json!([
                    "/usr/bin/zsh",
                    "-lc",
                    "python -m unittest tests.test_example || true"
                ]),
                "completed",
                json!(0),
                None,
            ),
            (
                json!(["/usr/bin/zsh", "-lc", "cargo test --no-run"]),
                "completed",
                json!(0),
                None,
            ),
            (
                json!(["some-wrapper", "-lc", "pytest tests"]),
                "completed",
                json!(0),
                None,
            ),
        ] {
            let mut item = json!({"type":"CommandExecution","id":"operation","command":command,"status":status});
            if !exit.is_null() {
                item["exit_code"] = exit;
            }
            let (facts, _) = translate(
                json!({"type":"item_completed","turn_id":"turn-1","item":item}),
                &context,
            );
            assert_eq!(
                facts
                    .iter()
                    .find(|fact| fact["kind"] == "check")
                    .and_then(|fact| fact["payload"]["result"].as_str()),
                expected
            );
            if expected.is_some() {
                assert_eq!(facts[0]["identity"], facts[1]["identity"]);
            }
        }
    }

    #[test]
    fn check_modes_require_validation_and_keep_read_only_lints_typed() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        for (command, status, exit, expected) in [
            ("cargo fmt --all -- --check", "completed", 0, Some("passed")),
            ("ruff format --check tests", "failed", 1, Some("failed")),
            ("git diff --cached --check", "completed", 0, Some("passed")),
            ("cargo fmt --all", "completed", 0, None),
            ("ruff format tests", "completed", 0, None),
            ("git diff --check --exit-code", "failed", 1, None),
            ("git diff --check --name-only", "completed", 0, None),
            ("pytest --fixtures", "completed", 0, None),
            ("pytest --markers", "completed", 0, None),
            ("go test -list=Test ./...", "completed", 0, None),
            ("go test -n ./...", "completed", 0, None),
            ("npm test --if-present", "completed", 0, None),
            ("cargo fmt --all -- --check", "failed", 256, None),
            ("ruff format --check tests && true", "completed", 0, None),
        ] {
            let (facts, _) = translate(
                json!({"type":"item_completed","turn_id":"turn-1",
                "item":{"type":"CommandExecution","id":"operation", "status":status,"exit_code":exit,
                    "command":["/usr/bin/zsh","-lc",command],"aggregated_output":"private result"}}),
                &context,
            );
            let check = facts.iter().find(|fact| fact["kind"] == "check");
            assert_eq!(
                check.and_then(|fact| fact["payload"]["result"].as_str()),
                expected,
                "{command}"
            );
            assert_eq!(facts[0]["kind"], "tool");
            if let Some(check) = check {
                assert_eq!(check["payload"]["category"], "lint");
                assert_eq!(check["identity"], facts[0]["identity"]);
            }
            assert!(
                !serde_json::to_string(&facts)
                    .unwrap()
                    .contains("private result")
            );
        }
    }

    #[test]
    fn command_check_and_usage_are_typed_without_source_content() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let (facts, _) = translate(
            json!({"type":"item_completed","turn_id":"turn-1","item":{
            "type":"CommandExecution","id":"check-1","command":["/usr/bin/zsh","-lc","npm run check"],
            "status":"completed","exit_code":0,"aggregated_output":"PRIVATE OUTPUT"}}),
            &context,
        );
        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts[0]["payload"],
            json!({"category":"shell","result":"succeeded"})
        );
        assert_eq!(
            facts[1]["payload"],
            json!({"category":"lint","result":"passed"})
        );
        assert!(facts[0]["identity"]["operation_id"].as_str().unwrap().len() == 64);
        assert!(!serde_json::to_string(&facts).unwrap().contains("PRIVATE"));
        assert!(!serde_json::to_string(&facts).unwrap().contains("npm"));
        let raw = row(
            "token_usage_record",
            json!({"turn_id":"turn-1","response_id":"response-1",
            "usage":{"input_tokens":7,"output_tokens":3,"cached_input_tokens":2,"reasoning_output_tokens":1}}),
        );
        let (usage, _) = translate_line(&raw, &context, "generation", 20).unwrap();
        assert_eq!(
            usage[0]["payload"],
            json!({"input_tokens":7,"output_tokens":3,"cache_read_tokens":2})
        );
        assert!(
            !serde_json::to_string(&usage)
                .unwrap()
                .contains("response-1")
        );
    }

    #[test]
    fn pull_request_requires_a_native_successful_create_and_an_exact_result() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let mut item = json!({
            "type": "CommandExecution",
            "id": "opaque-call",
            "command": ["/usr/bin/zsh", "-lc", "gh pr create --title 'private; title'"],
            "status": "completed",
            "exit_code": 0,
            "aggregated_output": "https://github.com/example/agenturm/pull/42\n"
        });
        let event =
            |item: &Value| json!({"type":"item_completed", "turn_id":"turn-1", "item":item});
        let (facts, _) = translate(event(&item), &context);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[1]["kind"], "pull_request_raised");
        assert_eq!(
            facts[1]["payload"],
            json!({"repository":"example/agenturm","number":42})
        );
        assert_eq!(facts[1]["identity"]["turn_id"], "turn-1");
        assert_eq!(
            facts[1]["identity"]["operation_id"],
            facts[0]["identity"]["operation_id"]
        );
        let encoded = serde_json::to_string(&facts).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("https://"));

        let mut unanchored = event(&item);
        unanchored.as_object_mut().unwrap().remove("turn_id");
        assert_eq!(translate(unanchored, &context).0.len(), 1);

        item["exit_code"] = json!(1);
        assert_eq!(translate(event(&item), &context).0.len(), 1);
        item["exit_code"] = json!(0);
        for command in [
            "echo https://github.com/example/agenturm/pull/42",
            "gh pr create --dry-run",
            "gh pr create && echo https://github.com/example/agenturm/pull/42",
            "gh pr create --body \"$(echo hi)\"",
        ] {
            item["command"] = json!(["bash", "-lc", command]);
            assert_eq!(translate(event(&item), &context).0.len(), 1, "{command}");
        }

        item["command"] = json!(["gh", "pr", "create"]);
        for output in [
            "https://github.com/example/agenturm/pull/42?extra=1",
            "https://github.com/example/agenturm/pull/042",
            "https://github.com/example/../pull/42",
            "https://github.com/example/agenturm/pull/42\nprivate extra output",
            "https://other.example/example/agenturm/pull/42",
        ] {
            item["aggregated_output"] = json!(output);
            assert_eq!(translate(event(&item), &context).0.len(), 1, "{output}");
        }
    }

    #[test]
    fn absent_result_flag_never_becomes_success() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let raw = row(
            "response_item",
            json!({"type":"function_call_output","call_id":"call-1",
            "output":"private successful looking text"}),
        );
        let (facts, _) = translate_line(&raw, &context, "generation", 10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["payload"]["code"], "codex_tool_result_unknown");
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn malformed_identity_usage_and_internal_review_fail_visibly() {
        assert!(
            header_identity(br#"{"type":"session_meta","payload":{"id":"../private"}}"#).is_err()
        );
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let bad_usage = row(
            "token_usage_record",
            json!({"response_id":"r","usage":{"input_tokens":"7"}}),
        );
        assert!(translate_line(&bad_usage, &context, "generation", 1).is_err());
        let bad_turn = row(
            "event_msg",
            json!({"type":"task_started","turn_id":"../private"}),
        );
        assert!(translate_line(&bad_turn, &context, "generation", 1).is_err());
        let internal = row("turn_context", json!({"model":"codex-auto-review"}));
        assert!(translate_line(&internal, &context, "generation", 1).is_err());
        let bad_result = row(
            "response_item",
            json!({"type":"function_call_output",
            "call_id":"c1","isError":"false"}),
        );
        assert!(translate_line(&bad_result, &context, "generation", 1).is_err());
    }

    #[test]
    fn guardian_rows_keep_scope_and_lifecycle_without_private_content_or_decisions() {
        let header = row(
            "session_meta",
            json!({"id":"guardian-thread","session_id":"parent-thread",
                "parent_thread_id":"parent-thread","thread_source":"guardian_review",
                "source":{"subagent":{"other":"guardian"}}}),
        );
        let context = header_identity(&header).unwrap();
        let (facts, _) = translate_line(&header, &context, "generation", 0).unwrap();
        assert_eq!(facts[0]["kind"], "internal_review");
        assert!(facts[0].get("identity").is_none());
        assert_eq!(
            facts[0]["payload"],
            json!({
                "state":"classified","session_id":"guardian-thread",
                "worker_id":"guardian-thread","parent_session_id":"parent-thread",
            })
        );
        let (facts, next) = translate(
            json!({"type":"task_started","turn_id":"review-turn"}),
            &context,
        );
        assert_eq!(facts[0]["payload"]["state"], "started");
        assert_eq!(facts[0]["payload"]["turn_id"], "review-turn");
        let mut resumed = header_identity(&header).unwrap();
        resumed.restore(&next.checkpoint());
        assert_eq!(resumed, next);
        for payload in [
            json!({"type":"item_completed","turn_id":"review-turn","item":{
                "type":"AgentMessage","phase":"final_answer","text":"approved private"},
            }),
            json!({"type":"approval_request","turn_id":"review-turn","request_id":"private"}),
            json!({"type":"task_complete","last_agent_message":"approved private"}),
            json!({"type":"task_complete","turn_id":"review-turn","error":{"message":"private"}}),
        ] {
            assert!(translate(payload, &resumed).0.is_empty());
        }
        let (facts, _) = translate(
            json!({"type":"task_complete","turn_id":"review-turn",
            "last_agent_message":"approved private"}),
            &resumed,
        );
        assert_eq!(facts[0]["payload"]["state"], "completed");
        assert_eq!(facts[0]["payload"]["turn_id"], "review-turn");
        assert!(!facts[0].to_string().contains("approved"));
        assert!(!facts[0].to_string().contains("private"));
        let (facts, _) = translate(
            json!({"type":"turn_aborted","turn_id":"previous-turn",
            "reason":"private"}),
            &resumed,
        );
        assert_eq!(facts[0]["payload"]["turn_id"], "previous-turn");
        assert_eq!(facts[0]["payload"]["state"], "interrupted");
        assert!(!facts[0].to_string().contains("private"));
    }

    #[test]
    fn native_requests_require_exact_identity_and_never_invent_resolution() {
        let context = header_identity(&row(
            "session_meta",
            json!({
                "id":"native-thread","session_id":"parent-thread",
            }),
        ))
        .unwrap();
        let (_, current) = translate(
            json!({"type":"task_started","turn_id":"active-turn"}),
            &context,
        );
        let request = json!({"type":"approval_request","turn_id":"previous-turn",
            "request_id":"approval-one","prompt":"private sentinel"});
        let (facts, next) = translate(request, &current);
        assert_eq!(next.turn_id, "active-turn");
        assert_eq!(facts[0]["kind"], "native_request");
        assert_eq!(
            facts[0]["identity"],
            json!({"session_id":"native-thread",
            "worker_id":"native-thread","turn_id":"previous-turn"})
        );
        assert_eq!(
            facts[0]["payload"],
            json!({"request_kind":"approval","state":"requested",
            "request_id":opaque_operation("approval-one").unwrap()})
        );
        assert!(!facts[0].to_string().contains("private sentinel"));
        assert_eq!(facts[1]["payload"]["code"], "codex_historical_wait");
        for payload in [
            json!({"type":"approval_request","turn_id":"active-turn","call_id":"not-a-request-id"}),
            json!({"type":"approval_request","request_id":"no-turn"}),
            json!({"type":"approval_response","turn_id":"active-turn","request_id":"approval-one","decision":"approve"}),
            json!({"type":"item_completed","turn_id":"active-turn","item":{
                "type":"AgentMessage","phase":"final_answer","text":"approved"},
            }),
        ] {
            assert!(
                translate(payload, &current)
                    .0
                    .iter()
                    .all(|row| row["kind"] != "native_request")
            );
        }
        let (facts, _) = translate(
            json!({"type":"request_user_input","turn_id":"active-turn",
            "request_id":"input-one","questions":["private"]}),
            &current,
        );
        assert_eq!(facts[0]["payload"]["request_kind"], "user_input");
        assert_eq!(facts[0]["payload"]["state"], "requested");
        assert!(!facts[0].to_string().contains("private"));
    }

    #[test]
    fn duplicate_scope_fields_cannot_create_a_request_or_hide_a_guardian() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread"}}"#).unwrap();
        let request = br#"{"type":"event_msg","timestamp":"2026-11-14T10:00:00Z","payload":{
            "type":"approval_request","turn_id":"turn","request_id":"one","request_id":"two"}}"#;
        let (facts, _) = translate_line(request, &context, "generation", 10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["kind"], "lifecycle");
        for header in [
            br#"{"type":"session_meta","payload":{"id":"thread","thread_source":"guardian_review","thread_source":"cli"}}"#.as_slice(),
            br#"{"type":"session_meta","payload":{"id":"thread","source":{"subagent":{"other":"guardian","other":"ordinary"}}}}"#.as_slice(),
        ] {
            assert!(header_identity(header).is_err());
        }
    }

    #[test]
    fn unmarked_new_user_message_does_not_inherit_old_turn() {
        let mut context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        context.turn_id = "old-turn".to_owned();
        let (facts, next) = translate(
            json!({"type":"item_completed","item":{
            "type":"UserMessage","content":"private new request"}}),
            &context,
        );
        assert!(next.turn_id.is_empty());
        assert!(facts[0]["identity"].get("turn_id").is_none());
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn worker_resumes_after_copied_parent_context() {
        let header = br#"{"type":"session_meta","payload":{"id":"worker","session_id":"parent","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}"#;
        let parent = br#"{"type":"session_meta","payload":{"id":"parent"}}"#;
        let context = header_identity(header).unwrap();
        let (facts, copied) =
            translate_line(parent, &context, "origin", header.len() as u64).unwrap();
        assert!(facts.is_empty());
        assert!(translate_line(parent, &context, "origin", header.len() as u64 + 1).is_err());
        let mut resumed = header_identity(header).unwrap();
        resumed.restore(&copied.checkpoint());

        let copied_start = row(
            "event_msg",
            json!({"type":"task_started","turn_id":"parent-turn"}),
        );
        let wrong_marker = row(
            "event_msg",
            json!({"type":"thread_settings_applied","thread_id":"parent"}),
        );
        for inherited in [&copied_start, &wrong_marker] {
            let (facts, state) = translate_line(inherited, &resumed, "origin", 1_000).unwrap();
            assert!(facts.is_empty());
            assert_eq!(state, resumed);
        }
        let worker_marker = row(
            "event_msg",
            json!({"type":"thread_settings_applied","thread_id":"worker"}),
        );
        let (facts, active) = translate_line(&worker_marker, &resumed, "origin", 2_000).unwrap();
        assert_eq!(facts[0]["payload"]["code"], "codex_inherited_prefix");

        let worker_start = row(
            "event_msg",
            json!({"type":"task_started","turn_id":"worker-turn"}),
        );
        let (facts, active) = translate_line(&worker_start, &active, "origin", 3_000).unwrap();
        assert_eq!(facts[0]["identity"]["session_id"], "worker");
        assert_eq!(facts[0]["identity"]["turn_id"], "worker-turn");
        assert_eq!(active.checkpoint(), "worker-turn");
    }

    #[test]
    fn late_old_turn_fact_keeps_row_scope_without_rewinding_checkpoint() {
        let mut current =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        current.turn_id = "turn-2".to_owned();
        let old_usage = row(
            "token_usage_record",
            json!({"turn_id":"turn-1",
            "response_id":"late-usage","usage":{"input_tokens":3}}),
        );
        let (usage_facts, after_usage) =
            translate_line(&old_usage, &current, "generation", 10).unwrap();
        assert_eq!(usage_facts[0]["identity"]["turn_id"], "turn-1");
        assert_eq!(after_usage.turn_id, "turn-2");
        let unmarked = row(
            "response_item",
            json!({"type":"function_call",
            "call_id":"call-new","name":"functions.exec"}),
        );
        let (tool_facts, after_tool) =
            translate_line(&unmarked, &after_usage, "generation", 20).unwrap();
        assert_eq!(tool_facts[0]["identity"]["turn_id"], "turn-2");
        assert_eq!(after_tool.turn_id, "turn-2");
        let old_terminal = row(
            "event_msg",
            json!({"type":"task_complete","turn_id":"turn-1"}),
        );
        let (terminal_facts, after_terminal) =
            translate_line(&old_terminal, &after_tool, "generation", 30).unwrap();
        assert_eq!(
            terminal_facts[0]["payload"]["code"],
            "codex_historical_outcome"
        );
        assert_eq!(after_terminal.turn_id, "turn-2");
        let old_final = row(
            "event_msg",
            json!({"type":"item_completed","turn_id":"turn-1",
            "item":{"type":"AgentMessage","phase":"final_answer","content":"private old answer"}}),
        );
        let (final_facts, after_final) =
            translate_line(&old_final, &after_terminal, "generation", 40).unwrap();
        assert_eq!(final_facts[0]["identity"]["turn_id"], "turn-1");
        assert_eq!(
            final_facts[1]["payload"]["code"],
            "codex_historical_outcome"
        );
        assert_eq!(after_final.turn_id, "turn-2");
    }
}
