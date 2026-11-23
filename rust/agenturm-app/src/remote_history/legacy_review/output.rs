use super::projection::safe_text;
use crate::handoff_export::{html_sections, markdown_sections};
use crate::review_receipt::ReviewFormat;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const TITLE: &str = "Agenturm execution evidence";
const EXPORT_ERROR: &str =
    "Cannot publish private Review files; choose unused destinations in an existing directory";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn output_extension(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension {
        Some(extension) if matches!(extension.as_str(), "md" | "html" | "json") => Ok(extension),
        _ => Err("Choose a new .md, .html or .json destination for Python Review".into()),
    }
}

pub(super) fn publish(
    projection: &Value,
    format: ReviewFormat,
    output: Option<&Path>,
    verify: impl Fn() -> Result<(), String>,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    let text = render(projection);
    let json = || encode(projection);
    let sections = || {
        Ok::<_, String>([
            ("Captured evidence", text.clone()),
            ("Source references", encode(&references(projection))?),
            (
                "Frozen scope and capture",
                encode(&json!({
                    "format": projection["format"], "scope": projection["scope"],
                    "captured_at": projection["captured_at"], "window": projection["window"],
                }))?,
            ),
        ])
    };
    let (mut markdown, mut html, mut encoded) = (None, None, None);
    if output.is_some() || matches!(format, ReviewFormat::Markdown | ReviewFormat::Html) {
        let sections = sections()?;
        if output.is_some() || format == ReviewFormat::Markdown {
            markdown = Some(markdown_sections(TITLE, &sections));
        }
        if output.is_some() || format == ReviewFormat::Html {
            html = Some(html_sections(TITLE, &sections));
        }
    }
    let output_kind = output.map(output_extension).transpose()?;
    if output_kind.as_deref() == Some("json") || format == ReviewFormat::Json {
        encoded = Some(json()?);
    }
    if let Some(path) = output {
        let documents = [
            ("md", markdown.as_deref()),
            ("html", html.as_deref()),
            (
                "json",
                encoded
                    .as_deref()
                    .filter(|_| output_kind.as_deref() == Some("json")),
            ),
        ];
        verify()?;
        let staged = StagedReview::new(path, &documents)?;
        verify()?;
        staged.publish()?;
        eprintln!("Wrote private Review files. Exports may contain captured execution text.");
    }
    let body = match format {
        ReviewFormat::Text => &text,
        ReviewFormat::Json => encoded.as_ref().ok_or("Cannot encode Review JSON")?,
        ReviewFormat::Markdown => markdown.as_ref().ok_or("Cannot encode Review Markdown")?,
        ReviewFormat::Html => html.as_ref().ok_or("Cannot encode Review HTML")?,
    };
    verify()?;
    stdout
        .write_all(body.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|_| "Cannot write private Review output".into())
}

fn encode(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|mut encoded| {
            encoded.push('\n');
            encoded
        })
        .map_err(|_| "Cannot encode Python Review".into())
}

fn references(projection: &Value) -> Value {
    let mut records = Vec::new();
    for name in [
        "request",
        "agent_update",
        "checks",
        "edits",
        "errors",
        "context_events",
        "unclassified_commands",
    ] {
        let value = &projection[name];
        let mut add = |row: &Value| {
            if let Some(source) = row.get("source") {
                records.push(json!({ "kind": name, "source": source }));
            }
        };
        if let Some(rows) = value.as_array() {
            for row in rows {
                add(row);
            }
        } else {
            add(value);
        }
    }
    json!(records)
}

fn render(projection: &Value) -> String {
    let scope = &projection["scope"];
    let window = &projection["window"];
    let checks = sequence(&projection["checks"]);
    let count = |status: &str| {
        checks
            .iter()
            .filter(|row| row["result"].as_str() == Some(status))
            .count()
    };
    let mut text = format!(
        "{}\n\nScope: {} · {}\nAgent reported: {}\nEvidence: {} · {} captured records\nChecks: {} passed · {} failed · {} unknown\n",
        label(&projection["title"]),
        label(&scope["kind"]),
        if label(&scope["turn_id"]).is_empty() {
            label(&scope["run_id"])
        } else {
            label(&scope["turn_id"])
        },
        label(&projection["reported_outcome"]),
        label(&window["coverage"]),
        window["count"],
        count("passed"),
        count("failed"),
        count("unknown"),
    );
    text.push_str("\nLATEST CAPTURED REQUEST FOR THIS RUN\n");
    let request = &projection["request"];
    if request.is_null() {
        text.push_str("Request unavailable in loaded evidence.\n");
    } else {
        line(
            &mut text,
            label(&request["text"]),
            request["clipped"].as_bool() == Some(true),
        );
        let context: Vec<_> = sequence(&request["context_omitted"])
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if !context.is_empty() {
            let _ = writeln!(text, "Captured context separated: {}", context.join(", "));
        }
    }
    if let Some(update) = projection["agent_update"]["text"].as_str() {
        text.push_str("\nAGENT-REPORTED UPDATE\n");
        line(
            &mut text,
            update,
            projection["agent_update"]["clipped"].as_bool() == Some(true),
        );
    }
    text.push_str("\nRECORDED CHECKS\n");
    for check in checks {
        let _ = writeln!(
            text,
            "{} · {}\n  {} · {}",
            label(&check["result"]).to_ascii_uppercase(),
            label(&check["command"]),
            label(&check["result_basis"]),
            label(&check["freshness"])
        );
    }
    if checks.is_empty() {
        text.push_str("No recognized validation commands in the loaded evidence.\n");
    }
    let unclassified = sequence(&projection["unclassified_commands"]);
    if !unclassified.is_empty() {
        let _ = writeln!(text, "\n{} UNCLASSIFIED SHELL RECORDS", unclassified.len());
        for row in unclassified.iter().take(12) {
            let _ = writeln!(
                text,
                "{} · {}",
                label(&row["command"]),
                label(&row["result"])
            );
        }
    }
    text.push_str("\nRECORDED MODIFYING OPERATIONS\n");
    for edit in sequence(&projection["edits"]) {
        let _ = writeln!(
            text,
            "{} · {} · {} · {}",
            label(&edit["action"]),
            label(&edit["path"]),
            label(&edit["result"]),
            label(&edit["effect"])
        );
    }
    text.push_str("\nRECORDED ERRORS\n");
    for row in sequence(&projection["errors"]) {
        let _ = writeln!(
            text,
            "{} · {}",
            label(&row["subject"]),
            label(&row["result_basis"])
        );
        if let Some(detail) = row["detail"].as_str().filter(|detail| !detail.is_empty()) {
            line(
                &mut text,
                detail,
                row["detail_clipped"].as_bool() == Some(true),
            );
        }
    }
    let usage = &projection["usage"];
    let _ = writeln!(text, "\nRECORDED USAGE · {}", label(&usage["scope"]));
    for (key, display) in [
        ("input_tokens", "Input tokens"),
        ("output_tokens", "Output tokens"),
        ("cache_read_input_tokens", "Cached input tokens"),
        ("cache_write_input_tokens", "Cache write tokens"),
        ("known_cost_usd", "Reported USD"),
        ("cost_usd", "Reported USD"),
    ] {
        let values = &usage["values"];
        let value = &values[key];
        let reported = values[format!("{key}_reported")].as_bool();
        if !value.is_null()
            && reported != Some(false)
            && (key != "cost_usd" || values["known_cost_usd"].is_null())
        {
            let _ = writeln!(text, "{display}: {value}");
        }
    }
    let context = sequence(&projection["context_events"]);
    if !context.is_empty() {
        text.push_str("\nCONTEXT EVENTS\n");
        for row in context {
            let _ = writeln!(text, "{}", label(&row["subject"]));
        }
    }
    let _ = writeln!(
        text,
        "\nEVIDENCE SCOPE\nReceiver: {}\nProducer: {}\nSession: {}\nActor: {}\nTrace: {}\nTurn: {}",
        label(&scope["receiver"]),
        label(&scope["producer"]),
        label(&scope["session_id"]),
        label(&scope["agent_id"]),
        label(&scope["run_id"]),
        label(&scope["turn_id"])
    );
    for reason in sequence(&window["reasons"]) {
        let _ = writeln!(text, "{}", label(reason));
    }
    let _ = writeln!(
        text,
        "{}\n{}",
        label(&window["note"]),
        label(&usage["note"])
    );
    safe_text(&text)
}

fn label(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn sequence(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn line(text: &mut String, value: &str, clipped: bool) {
    text.push_str(value);
    if clipped {
        text.push_str("… (excerpt clipped)");
    }
    text.push('\n');
}

struct StagedReview {
    directory: PathBuf,
    files: Vec<(PathBuf, PathBuf)>,
}

impl StagedReview {
    fn new(path: &Path, documents: &[(&str, Option<&str>)]) -> Result<Self, String> {
        let name = path.file_name().ok_or(EXPORT_ERROR)?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent.canonicalize().map_err(|_| EXPORT_ERROR)?;
        let destination = parent.join(name);
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| EXPORT_ERROR)?
            .as_nanos();
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = parent.join(format!(
            ".agenturm-review-{nanoseconds}-{}-{sequence}.pending",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(unix))]
        let _ = &mut builder;
        builder.create(&directory).map_err(|_| EXPORT_ERROR)?;
        let mut staged = Self {
            directory,
            files: Vec::new(),
        };
        let extension = output_extension(path)?;
        for (suffix, content) in documents {
            let Some(content) = content else {
                continue;
            };
            let destination = if suffix == &extension {
                destination.clone()
            } else {
                destination.with_extension(suffix)
            };
            match fs::symlink_metadata(&destination) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(EXPORT_ERROR.into()),
            }
            let temporary = staged.directory.join(suffix);
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(|_| EXPORT_ERROR)?;
            file.write_all(content.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|_| EXPORT_ERROR)?;
            staged.files.push((temporary, destination));
        }
        Ok(staged)
    }

    fn publish(self) -> Result<(), String> {
        for (index, (source, destination)) in self.files.iter().enumerate() {
            if fs::hard_link(source, destination).is_err() {
                for (source, destination) in self.files.iter().take(index) {
                    if same_file(source, destination) {
                        let _ = fs::remove_file(destination);
                    }
                }
                return Err(EXPORT_ERROR.into());
            }
        }
        Ok(())
    }
}

impl Drop for StagedReview {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
fn same_file(source: &Path, destination: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(source), Ok(destination)) = (source.symlink_metadata(), destination.symlink_metadata())
    else {
        return false;
    };
    source.is_file()
        && destination.is_file()
        && source.dev() == destination.dev()
        && source.ino() == destination.ino()
}

#[cfg(not(unix))]
fn same_file(_source: &Path, _destination: &Path) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn changed_policy_after_staging_writes_no_receipt_or_stdout() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-review-policy-test-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let cleanup = Cleanup(directory);
        let seen = Cell::new(0);
        let verify = || {
            seen.set(seen.get() + 1);
            if seen.get() == 1 {
                Ok(())
            } else {
                Err("fixture policy revoked".into())
            }
        };
        let projection = json!({ "format": "agenturm-review-v1", "request": {
            "text": "<strong>protected fixture</strong>",
        }});
        let mut stdout = Vec::new();
        let result = publish(
            &projection,
            ReviewFormat::Json,
            Some(&cleanup.0.join("receipt.json")),
            verify,
            &mut stdout,
        );
        assert_eq!(result.unwrap_err(), "fixture policy revoked");
        assert_eq!(seen.get(), 2);
        assert!(stdout.is_empty());
        assert_eq!(fs::read_dir(&cleanup.0).unwrap().count(), 0);
    }
}
