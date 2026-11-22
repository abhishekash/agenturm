//! A bounded, content-free translation of one actual Pi session JSONL line.
//! Source text is parsed in the child and never placed in a process fact.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn operation_id(value: &str) -> Option<Cow<'_, str>> {
    if identifier(value) {
        return Some(Cow::Borrowed(value));
    }
    let (call, item) = value.split_once('|')?;
    let call_suffix = call.strip_prefix("call_")?;
    let item_suffix = item
        .strip_prefix("fc_")
        .or_else(|| item.strip_prefix("ctc_"))?;
    if call_suffix.is_empty() || item_suffix.is_empty() || !identifier(call) || !identifier(item) {
        return None;
    }
    // Pi combines the provider call and item IDs. Hash the pair into a
    // namespace containing a slash, which plain native Pi IDs cannot contain.
    Some(Cow::Owned(format!("pi-call/{:x}", Sha256::digest(value))))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    pub worker_id: String,
    pub turn_id: String,
    fork_parent: Option<String>,
}

pub(crate) fn header_context(path: Option<&Path>, raw: &[u8]) -> Result<Context> {
    let row: Value = serde_json::from_slice(raw)?;
    let mut context = header_from_row(&row)?;
    if let Some(path) = path
        && row.get("parentSessionId").is_none()
    {
        context.fork_parent = native_fork_parent(path, raw, &row, &context);
    }
    Ok(context)
}

fn header_from_row(row: &Value) -> Result<Context> {
    if row.get("type").and_then(Value::as_str) != Some("session") {
        return Err(invalid("Pi source must start with a session header"));
    }
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Pi session header lacks an ID")?;
    let session_id = row.get("sessionId").and_then(Value::as_str).unwrap_or(id);
    let worker_id = row.get("agentId").and_then(Value::as_str).unwrap_or("");
    if !identifier(id)
        || !identifier(session_id)
        || (!worker_id.is_empty() && !identifier(worker_id))
    {
        return Err(invalid("invalid Pi session identity"));
    }
    Ok(Context {
        session_id: session_id.to_owned(),
        worker_id: worker_id.to_owned(),
        turn_id: String::new(),
        fork_parent: None,
    })
}

fn native_fork_parent(path: &Path, raw: &[u8], row: &Value, child: &Context) -> Option<String> {
    let parent_reference = Path::new(row.get("parentSession")?.as_str()?);
    let directory = path.parent()?;
    if !path.is_absolute()
        || !parent_reference.is_absolute()
        || parent_reference.parent()? != directory
        || !child.worker_id.is_empty()
        || row.get("id")?.as_str()? != child.session_id
        || native_filename_id(path)? != child.session_id
    {
        return None;
    }
    let expected_parent = native_filename_id(parent_reference)?;
    if expected_parent == child.session_id {
        return None;
    }
    let mut parent = open_prefix(&directory.join(parent_reference.file_name()?))?;
    let parent_row: Value = serde_json::from_slice(&prefix_line(&mut parent)?).ok()?;
    let parent_header = header_from_row(&parent_row).ok()?;
    if parent_header.session_id != expected_parent
        || parent_row.get("id")?.as_str()? != expected_parent
        || !parent_header.worker_id.is_empty()
    {
        return None;
    }
    // A parent path is also used for an empty continuation. Require a copied
    // entry to distinguish a fork and retain only the header's native ID.
    let mut current = open_prefix(path)?;
    if prefix_line(&mut current)? != raw {
        return None;
    }
    let copied = prefix_line(&mut parent)?;
    if copied != prefix_line(&mut current)? {
        return None;
    }
    let entry: Value = serde_json::from_slice(&copied).ok()?;
    entry.get("id")?.as_str().filter(|id| identifier(id))?;
    Some(parent_header.session_id)
}

fn native_filename_id(path: &Path) -> Option<&str> {
    let (stamp, id) = path
        .file_name()?
        .to_str()?
        .strip_suffix(".jsonl")?
        .rsplit_once('_')?;
    // Pi names these files with an ISO timestamp with colons and the decimal
    // point replaced by dashes, followed by the session UUID.
    let mut stamp: [u8; 24] = stamp.as_bytes().try_into().ok()?;
    if [13, 16, 19].into_iter().any(|index| stamp[index] != b'-') || stamp[23] != b'Z' {
        return None;
    }
    [stamp[13], stamp[16], stamp[19]] = *b"::.";
    OffsetDateTime::parse(std::str::from_utf8(&stamp).ok()?, &Rfc3339).ok()?;
    (id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }))
    .then_some(id)
}

fn open_prefix(path: &Path) -> Option<BufReader<File>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    file.metadata()
        .ok()?
        .is_file()
        .then(|| BufReader::new(file))
}

fn prefix_line(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    const MAX_PREFIX_LINE: usize = 64 * 1024;
    let mut line = Vec::new();
    reader
        .take((MAX_PREFIX_LINE + 1) as u64)
        .read_until(b'\n', &mut line)
        .ok()?;
    (line.len() <= MAX_PREFIX_LINE && line.ends_with(b"\n")).then_some(line)
}

fn identity(context: &Context, operation: Option<&str>) -> Value {
    let mut result = json!({"session_id":context.session_id});
    if !context.worker_id.is_empty() {
        result["worker_id"] = json!(context.worker_id);
    }
    if !context.turn_id.is_empty() {
        result["turn_id"] = json!(context.turn_id);
    }
    if let Some(operation) = operation {
        result["operation_id"] = json!(operation);
    }
    result
}

fn fact(
    event_id: String,
    kind: &str,
    time_ns: &str,
    context: &Context,
    payload: Value,
    operation: Option<&str>,
) -> Value {
    json!({"event_id":event_id,"revision":0,"kind":kind,"time_ns":time_ns,
           "identity":identity(context, operation),"payload":payload})
}

fn stamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Pi row lacks timestamp")?;
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)?;
    let nanos = parsed.unix_timestamp_nanos();
    if nanos < 0 || nanos > i64::MAX as i128 {
        return Err(invalid("Pi timestamp exceeds supported range"));
    }
    Ok(nanos.to_string())
}

fn usage(message: &Value) -> Result<Option<Value>> {
    let Some(raw) = message.get("usage") else {
        return Ok(None);
    };
    let fields = raw.as_object().ok_or("Pi usage is not an object")?;
    let mut result = serde_json::Map::new();
    for (source, target) in [
        ("input", "input_tokens"),
        ("output", "output_tokens"),
        ("cacheRead", "cache_read_tokens"),
        ("cacheWrite", "cache_write_tokens"),
    ] {
        if let Some(value) = fields.get(source) {
            let count = value.as_u64().ok_or("invalid Pi usage counter")?;
            if count > i64::MAX as u64 {
                return Err(invalid("Pi usage counter exceeds supported range"));
            }
            result.insert(target.to_owned(), json!(count));
        }
    }
    Ok((!result.is_empty()).then_some(Value::Object(result)))
}

fn category(name: &str) -> &'static str {
    if name.starts_with("mcp__") {
        "mcp"
    } else if matches!(name, "read" | "write" | "edit" | "Read" | "Write" | "Edit") {
        "file"
    } else if matches!(name, "bash" | "shell" | "exec" | "Bash" | "Shell") {
        "shell"
    } else {
        "other"
    }
}

// Only a direct check command is attributed to this row's shell exit.
fn shell_check(command: &str) -> Option<&'static str> {
    if command.len() > 4096
        || !command.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b' ' | b'\t' | b'/' | b'.' | b'_' | b'-' | b':')
        })
    {
        return None;
    }
    let mut words = command.split_ascii_whitespace();
    let program = Path::new(words.next()?).file_name()?.to_str()?;
    let args = words.collect::<Vec<_>>();
    if args.iter().any(|word| {
        matches!(
            *word,
            "-h" | "--help"
                | "--version"
                | "--collect-only"
                | "--co"
                | "--dry-run"
                | "--list"
                | "--no-run"
        )
    }) {
        return None;
    }
    match (program, args.as_slice()) {
        ("cargo" | "go", ["test", ..])
        | ("pytest", _)
        | ("python" | "python3", ["-m", "pytest" | "unittest", ..]) => Some("test"),
        ("cargo", ["clippy", ..]) | ("ruff", ["check", ..]) => Some("lint"),
        ("cargo" | "go", ["build", ..]) => Some("build"),
        _ => None,
    }
}

/// Returns all facts for one source line and the context to checkpoint only
/// when the caller accepts that entire vector into its poll batch.
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
        .ok_or("Pi row lacks type")?;
    let mut next = context.clone();
    if kind == "session" {
        let header = header_from_row(&row)?;
        if header.session_id != context.session_id || header.worker_id != context.worker_id {
            return Err(invalid("Pi session identity changed inside one file"));
        }
        let relationship = row
            .get("parentSessionId")
            .and_then(Value::as_str)
            .filter(|parent| identifier(parent) && *parent != header.session_id)
            .map(|parent| ("delegated", parent))
            .or_else(|| {
                context
                    .fork_parent
                    .as_deref()
                    .map(|parent| ("forked", parent))
            });
        if offset == 0
            && let Some((edge, parent)) = relationship
            && let Ok(time_ns) = stamp(&row)
        {
            return Ok((
                vec![fact(
                    format!("pi:{origin}:{offset}:relationship"),
                    "relationship",
                    &time_ns,
                    &header,
                    json!({"relationship_kind":edge,"parent_session_id":parent}),
                    None,
                )],
                next,
            ));
        }
        return Ok((vec![], next));
    }
    let row_id = row.get("id").and_then(Value::as_str);
    let base = if let Some(id) = row_id {
        if !identifier(id) {
            return Err(invalid("invalid Pi row ID"));
        }
        format!("pi:{origin}:{id}")
    } else {
        format!("pi:{origin}:offset-{offset}")
    };
    if base.len() > 230 {
        return Err(invalid("Pi event identity exceeds wire limit"));
    }
    if kind != "message" {
        // Unsupported Pi metadata is visible as a coarse diagnostic. An
        // adapter never claims complete coverage by silently skipping it.
        let time_ns = row
            .get("timestamp")
            .map_or(Ok("0".to_owned()), |_| stamp(&row))?;
        return Ok((
            vec![
                json!({"event_id":format!("{base}:unsupported"),"revision":0,
            "kind":"diagnostic","time_ns":time_ns,
            "payload":{"code":"unsupported_pi_row","severity":"info"}}),
            ],
            next,
        ));
    }
    let time_ns = stamp(&row)?;
    let message = row
        .get("message")
        .and_then(Value::as_object)
        .ok_or("Pi message is not an object")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or("Pi message lacks role")?;
    let mut facts = Vec::new();
    match role {
        "user" => {
            let id = row_id.ok_or("Pi user message lacks ID")?;
            next.turn_id = id.to_owned();
            facts.push(fact(
                format!("{base}:message"),
                "message",
                &time_ns,
                &next,
                json!({"role":"user"}),
                None,
            ));
            facts.push(fact(
                format!("{base}:lifecycle"),
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"running"}),
                None,
            ));
        }
        "assistant" => {
            row_id.ok_or("Pi assistant message lacks ID")?;
            facts.push(fact(
                format!("{base}:message"),
                "message",
                &time_ns,
                &next,
                json!({"role":"assistant"}),
                None,
            ));
            if let Some(counters) = usage(&row["message"])? {
                facts.push(fact(
                    format!("{base}:usage"),
                    "usage",
                    &time_ns,
                    &next,
                    counters,
                    None,
                ));
            }
            if let Some(content) = message.get("content") {
                let blocks = content
                    .as_array()
                    .ok_or("Pi assistant content is not an array")?;
                if blocks.len() > 128 {
                    return Err(invalid("Pi assistant content block limit exceeded"));
                }
                for (index, block) in blocks.iter().enumerate() {
                    if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                        continue;
                    }
                    let native_operation = block
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or("Pi tool call lacks ID")?;
                    let operation =
                        operation_id(native_operation).ok_or("invalid Pi tool call identity")?;
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    facts.push(fact(
                        format!("{base}:tool-{index}"),
                        "tool",
                        &time_ns,
                        &next,
                        json!({"category":category(name),"result":"started"}),
                        Some(&operation),
                    ));
                }
            }
            match message.get("stopReason").and_then(Value::as_str) {
                Some("stop") => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"completed"}),
                    None,
                )),
                Some("error") => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"interrupted","reason_code":"pi_stop_error"}),
                    None,
                )),
                Some("aborted") => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"interrupted","reason_code":"pi_stop_aborted"}),
                    None,
                )),
                _ => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"running"}),
                    None,
                )),
            }
        }
        "toolResult" => {
            row_id.ok_or("Pi tool result lacks ID")?;
            let native_operation = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or("Pi tool result lacks call ID")?;
            let operation =
                operation_id(native_operation).ok_or("invalid Pi tool result identity")?;
            let failed = message
                .get("isError")
                .and_then(Value::as_bool)
                .ok_or("Pi tool result lacks explicit isError")?;
            let name = message
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("");
            // The current user turn may have advanced since this call began.
            // The host can relate a result to the original turn only from one
            // policy-current, admitted call fact with this operation ID.
            let mut result_context = next.clone();
            result_context.turn_id.clear();
            facts.push(fact(
                format!("{base}:tool-result"),
                "tool",
                &time_ns,
                &result_context,
                json!({"category":category(name),
                                          "result":if failed {"failed"} else {"succeeded"}}),
                Some(&operation),
            ));
            facts.push(fact(
                format!("{base}:lifecycle"),
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"running"}),
                None,
            ));
        }
        "bashExecution" => {
            // User-side shell commands can be queued while the model is
            // streaming. The row provides no exact model turn correlation.
            let mut shell_context = next.clone();
            shell_context.turn_id.clear();
            let operation = format!("pi-shell/{:x}", Sha256::digest(&base));
            let exit = message.get("exitCode").and_then(Value::as_i64);
            if message.get("cancelled").and_then(Value::as_bool) == Some(false)
                && let Some(exit) = exit.filter(|exit| *exit >= 0)
            {
                let (tool_result, check_result) = if exit == 0 {
                    ("succeeded", "passed")
                } else {
                    ("failed", "failed")
                };
                facts.push(fact(
                    format!("{base}:shell"),
                    "tool",
                    &time_ns,
                    &shell_context,
                    json!({"category":"shell","result":tool_result}),
                    Some(&operation),
                ));
                if let Some(check) = message
                    .get("command")
                    .and_then(Value::as_str)
                    .and_then(shell_check)
                {
                    facts.push(fact(
                        format!("{base}:check"),
                        "check",
                        &time_ns,
                        &shell_context,
                        json!({"category":check,"result":check_result}),
                        Some(&operation),
                    ));
                }
            } else {
                facts.push(
                    json!({"event_id":format!("{base}:shell-unknown"),"revision":0,
                    "kind":"diagnostic","time_ns":time_ns,
                    "payload":{"code":"unsupported_pi_role","severity":"info"}}),
                );
            }
        }
        _ => facts.push(
            json!({"event_id":format!("{base}:unsupported"),"revision":0,
            "kind":"diagnostic","time_ns":time_ns,
            "payload":{"code":"unsupported_pi_role","severity":"warning"}}),
        ),
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_identity(raw: &[u8]) -> Result<Context> {
        header_from_row(&serde_json::from_slice(raw)?)
    }

    #[test]
    fn composite_provider_operation_ids_pair_calls_and_results() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for native_id in ["call_abc|fc_xyz", "call_abc|ctc_xyz"] {
            let call = json!({
                "type":"message","id":"call","timestamp":"2026-10-24T12:00:00Z",
                "message":{"role":"assistant","stopReason":"toolUse",
                    "content":[{"type":"toolCall","id":native_id,"name":"read"}]}
            });
            let result = json!({
                "type":"message","id":"result","timestamp":"2026-10-24T12:00:01Z",
                "message":{"role":"toolResult","toolCallId":native_id,"toolName":"read","isError":false}
            });
            let (call_facts, next) =
                translate_line(call.to_string().as_bytes(), &context, "file", 0).unwrap();
            let (result_facts, _) =
                translate_line(result.to_string().as_bytes(), &next, "file", 1).unwrap();
            let [call, result] = [&call_facts, &result_facts]
                .map(|facts| facts.iter().find(|fact| fact["kind"] == "tool").unwrap());
            let id = call["identity"]["operation_id"].as_str().unwrap();
            assert!(id.starts_with("pi-call/"));
            assert_eq!(id, result["identity"]["operation_id"]);
            assert!(!call.to_string().contains(native_id));
            assert!(!result.to_string().contains(native_id));
        }
        assert_eq!(operation_id("plain-id").as_deref(), Some("plain-id"));
        for malformed in [
            "call_|fc_abc",
            "call_abc|fc_",
            "call_abc|other",
            "call_abc|fc_abc|fc_xyz",
        ] {
            assert!(operation_id(malformed).is_none());
        }
    }

    #[test]
    fn relationship_requires_an_explicit_distinct_parent_session_id() {
        let header = br#"{"type":"session","id":"child","timestamp":"2026-10-24T12:00:00Z","parentSessionId":"parent"}"#;
        let context = header_identity(header).unwrap();
        let (facts, _) = translate_line(header, &context, "file", 0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["identity"], json!({"session_id":"child"}));
        assert_eq!(facts[0]["kind"], "relationship");
        assert_eq!(
            facts[0]["payload"],
            json!({"relationship_kind":"delegated","parent_session_id":"parent"}),
        );
        assert!(
            translate_line(header, &context, "file", 10)
                .unwrap()
                .0
                .is_empty()
        );

        for (field, value) in [
            ("parentSession", "path/to/private.jsonl"),
            ("parentAgentId", "worker"),
            ("parentSessionId", "../private"),
            ("parentSessionId", "child"),
        ] {
            let mut raw = json!({"type":"session","id":"child","timestamp":"2026-10-24T12:00:00Z"});
            raw[field] = json!(value);
            let (facts, _) =
                translate_line(raw.to_string().as_bytes(), &context, "file", 0).unwrap();
            assert!(facts.is_empty());
        }
    }

    #[test]
    fn one_assistant_row_yields_usage_two_tools_and_no_false_completion() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        let (facts, next) = translate_line(br#"{"type":"message","id":"a-1","timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":8,"output":3},"content":[{"type":"toolCall","id":"c-1","name":"read","arguments":{"path":"secret"}},{"type":"toolCall","id":"c-2","name":"bash","arguments":{"command":"secret"}}]}}"#, &context, "file", 10).unwrap();
        assert_eq!(facts.len(), 5);
        assert_eq!(facts[1]["kind"], "usage");
        assert_eq!(facts[2]["identity"]["operation_id"], "c-1");
        assert_eq!(facts[3]["payload"]["category"], "shell");
        assert_eq!(facts[4]["payload"]["state"], "running");
        assert!(!serde_json::to_string(&facts).unwrap().contains("secret"));
        assert_eq!(next, context);
    }

    #[test]
    fn explicit_stop_and_error_are_terminal_but_missing_result_is_not_success() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for (stop, state) in [("stop", "completed"), ("error", "interrupted")] {
            let row = format!(
                r#"{{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{{"role":"assistant","stopReason":"{stop}"}}}}"#
            );
            let (facts, _) = translate_line(row.as_bytes(), &context, "file", 0).unwrap();
            assert_eq!(facts[1]["payload"]["state"], state);
        }
        assert!(translate_line(br#"{"type":"message","id":"t","timestamp":"2026-10-24T12:00:00Z","message":{"role":"toolResult","toolCallId":"c","toolName":"read"}}"#, &context, "file", 0).is_err());
    }

    #[test]
    fn malformed_identity_usage_and_time_fail_before_fact_emission() {
        assert!(header_identity(br#"{"type":"session","id":"../private"}"#).is_err());
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for raw in [
            br#"{"type":"message","id":"a","timestamp":"not-a-time","message":{"role":"assistant"}}"#.as_slice(),
            br#"{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{"role":"assistant","usage":{"input":-1}}}"#.as_slice(),
            br#"{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"../bad","name":"read"}]}}"#.as_slice(),
        ] {
            assert!(translate_line(raw, &context, "file", 0).is_err());
        }
    }

    #[test]
    fn explicit_pi_session_and_worker_ids_remain_distinct() {
        let context = header_identity(
            br#"{"type":"session","id":"file-id","sessionId":"session-1","agentId":"worker-1"}"#,
        )
        .unwrap();
        let (facts, next) = translate_line(br#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#, &context, "file", 0).unwrap();
        assert_eq!(facts[0]["identity"]["session_id"], "session-1");
        assert_eq!(facts[0]["identity"]["worker_id"], "worker-1");
        assert_eq!(next.turn_id, "user-1");
    }

    #[test]
    fn result_does_not_borrow_current_turn_from_a_later_user() {
        let context = Context {
            session_id: "pi-1".into(),
            turn_id: "user-2".into(),
            ..Context::default()
        };
        let (facts, next) = translate_line(
            br#"{"type":"message","id":"result-1","timestamp":"2026-10-24T12:00:03Z","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","isError":true,"content":"private output"}}"#,
            &context,
            "file",
            100,
        )
        .unwrap();
        assert_eq!(facts[0]["identity"]["operation_id"], "call-1");
        assert!(facts[0]["identity"].get("turn_id").is_none());
        assert_eq!(facts[1]["identity"]["turn_id"], "user-2");
        assert_eq!(next, context);
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn native_user_shell_checks_require_an_explicit_exit_and_remain_session_scoped() {
        let context = Context {
            session_id: "pi-session".into(),
            turn_id: "later-turn".into(),
            ..Context::default()
        };
        for (command, exit, cancelled, expected) in [
            (
                "python3 -m unittest test_probe",
                json!(0),
                false,
                Some("passed"),
            ),
            (
                "python3 -m unittest test_probe",
                json!(1),
                false,
                Some("failed"),
            ),
            ("python3 -m unittest test_probe", Value::Null, false, None),
            ("python3 -m unittest test_probe", json!(0), true, None),
            (
                "python3 -m unittest test_probe | cat",
                json!(0),
                false,
                None,
            ),
            ("cargo test --no-run", json!(0), false, None),
        ] {
            let row = json!({"type":"message","id":"bash-1","timestamp":"2026-11-20T12:00:00Z",
                "message":{"role":"bashExecution","command":command,"exitCode":exit,
                "cancelled":cancelled,"output":"private output"}});
            let (facts, next) =
                translate_line(&serde_json::to_vec(&row).unwrap(), &context, "origin", 40).unwrap();
            assert_eq!(next, context);
            assert_eq!(
                facts
                    .iter()
                    .find(|fact| fact["kind"] == "check")
                    .and_then(|fact| fact["payload"]["result"].as_str()),
                expected
            );
            if expected.is_some() {
                assert_eq!(facts[0]["identity"], facts[1]["identity"]);
                assert!(facts[1]["identity"].get("turn_id").is_none());
                assert_eq!(facts[1]["payload"]["category"], "test");
            }
            assert!(
                !serde_json::to_string(&facts)
                    .unwrap()
                    .contains("private output")
            );
            assert!(!serde_json::to_string(&facts).unwrap().contains(command));
        }
    }
}
