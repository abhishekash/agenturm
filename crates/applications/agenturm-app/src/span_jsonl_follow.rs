//! Continuous ingestion of an explicitly configured span JSONL file.

use super::span_jsonl_import::{self, SpanJsonlImportOptions};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;

pub(super) const USAGE: &str = concat!(
    "agenturm watch-jsonl --file <span-jsonl-path> --db <path> --source-id otlp.<id> ",
    "[--session-id <id>] [--interval-ms <100..60000>] ",
    "[--payload-scope <standalone|harness-id> [--payload-format agent-harness]]"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanJsonlFollowOptions {
    import: SpanJsonlImportOptions,
    interval: Duration,
}

pub(super) fn parse(args: &[String]) -> Result<SpanJsonlFollowOptions, String> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    let mut importer = Vec::new();
    let mut interval = None;
    for [flag, value] in pairs {
        if flag == "--interval-ms" {
            if interval.is_some() {
                return Err(USAGE.into());
            }
            interval = Some(Duration::from_millis(
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| (100..=60_000).contains(value))
                    .ok_or(USAGE)?,
            ));
        } else {
            importer.extend([flag.clone(), value.clone()]);
        }
    }
    let import = span_jsonl_import::parse(&importer).map_err(|_| USAGE)?;
    if import.file.as_os_str() == "-" {
        return Err(USAGE.into());
    }
    Ok(SpanJsonlFollowOptions {
        import,
        interval: interval.unwrap_or(Duration::from_millis(500)),
    })
}

pub fn execute_span_jsonl_follow(options: &SpanJsonlFollowOptions) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::execute(options)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        Err("Watching span JSONL requires Linux".into())
    }
}
