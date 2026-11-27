//! Bounded Review receipts through an explicitly selected native receiver.

use crate::remote_history::legacy_review;
use crate::remote_history::{self, RemoteHistoryOptions, RemoteStore};
use crate::review_receipt::ReviewFormat;
use crate::{Command, HistoryQueryOptions, ReviewOutput, ReviewSelection};
use std::collections::BTreeMap;
use std::path::PathBuf;

const LEGACY_USAGE: &str = "agenturm review --receiver-protocol python-v0.2.8 --endpoint http://<loopback-IP:port> --token-file <path> --service <name> [--agent-id <id>] [--session-id <id>] (--trace-id <id> | --turn-of-trace <id>) [--pages 1..5] [--format text|json|markdown|html] [-o <new.md|html|json>]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyReviewOptions {
    pub(crate) receiver: RemoteHistoryOptions,
    pub(crate) service: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) trace_id: String,
    pub(crate) whole_turn: bool,
    pub(crate) pages: usize,
    pub(crate) format: ReviewFormat,
    pub(crate) output: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteReviewOptions {
    receiver: RemoteHistoryOptions,
    query: HistoryQueryOptions,
    selection: ReviewSelection,
    output: ReviewOutput,
}

pub(crate) fn parse(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|arg| arg == "--receiver-protocol") {
        return parse_python(args).map(|options| Command::LegacyReview(Box::new(options)));
    }
    if !args.iter().any(|arg| arg == "--endpoint") {
        return crate::parse_query_options(args, true);
    }
    let usage = "agenturm review --endpoint http://<loopback-IP:port> --token-file <path> --harness-id <id> --session-id <id> (--turn-id <id> | --turn-of-trace <trace-id> --trace-source-id <id>) [Review paging/output options]";
    let mut remote = Vec::new();
    // The sentinel is only a parser value. No local history is opened.
    let mut local = vec!["--db".into(), "remote-review-unused".into()];
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if matches!(flag.as_str(), "--db" | "--receiver-protocol") {
            return Err(usage.into());
        }
        if flag == "--linked-traces" {
            local.push(flag.clone());
            index += 1;
            continue;
        }
        let value = args.get(index + 1).ok_or(usage)?;
        let target = if matches!(flag.as_str(), "--endpoint" | "--token-file") {
            &mut remote
        } else {
            &mut local
        };
        target.extend([flag.clone(), value.clone()]);
        index += 2;
    }
    let receiver = remote_history::parse_attach(&remote)?;
    let Command::Review {
        query,
        selection,
        output,
    } = crate::parse_query_options(&local, true)?
    else {
        return Err(usage.into());
    };
    Ok(Command::RemoteReview(RemoteReviewOptions {
        receiver,
        query,
        selection,
        output,
    }))
}

fn parse_python(args: &[String]) -> Result<LegacyReviewOptions, String> {
    let (pairs, rest) = args.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(LEGACY_USAGE.into());
    }
    let mut receiver_args = Vec::new();
    let mut fields = BTreeMap::new();
    for [flag, value] in pairs {
        if value.is_empty() || value.starts_with("--") {
            return Err(LEGACY_USAGE.into());
        }
        match flag.as_str() {
            "--receiver-protocol" | "--endpoint" | "--token-file" => {
                receiver_args.extend([flag.clone(), value.clone()]);
                continue;
            }
            "--service" | "--agent-id" | "--session-id" | "--trace-id" | "--turn-of-trace"
            | "--pages" | "--format" | "-o" | "--output" => {}
            _ => return Err(LEGACY_USAGE.into()),
        }
        let key = if flag == "-o" { "--output" } else { flag };
        if fields.insert(key, value.as_str()).is_some() {
            return Err(LEGACY_USAGE.into());
        }
    }
    let receiver = remote_history::parse_attach(&receiver_args)?;
    let service = fields.remove("--service").ok_or(LEGACY_USAGE)?;
    let agent = fields.remove("--agent-id").unwrap_or_default();
    let session = fields.remove("--session-id").unwrap_or_default();
    let (trace, whole_turn) = match (
        fields.remove("--trace-id"),
        fields.remove("--turn-of-trace"),
    ) {
        (Some(trace), None) => (trace, false),
        (None, Some(trace)) if service == "opencode" => (trace, true),
        _ => return Err(LEGACY_USAGE.into()),
    };
    if !receiver.is_python()
        || (agent.is_empty() && session.is_empty())
        || [service, agent, session]
            .into_iter()
            .any(|id| !legacy_review::valid_identity(id))
        || !legacy_review::valid_trace(trace)
    {
        return Err(LEGACY_USAGE.into());
    }
    let pages = fields
        .remove("--pages")
        .unwrap_or("1")
        .parse::<usize>()
        .ok()
        .filter(|pages| (1..=5).contains(pages))
        .ok_or(LEGACY_USAGE)?;
    let format = ReviewFormat::parse(fields.remove("--format").unwrap_or("text"))?;
    let output = fields.remove("--output").map(PathBuf::from);
    if let Some(path) = &output {
        legacy_review::output_extension(path)?;
    }
    Ok(LegacyReviewOptions {
        receiver,
        service: service.into(),
        agent_id: agent.into(),
        session_id: session.into(),
        trace_id: trace.into(),
        whole_turn,
        pages,
        format,
        output,
    })
}

pub fn execute_legacy_review(options: &LegacyReviewOptions) -> Result<(), String> {
    crate::private_output::with_private_stdout(|stdout| legacy_review::execute(options, stdout))
}

pub fn execute_remote_review(options: &RemoteReviewOptions) -> Result<String, String> {
    let store = RemoteStore::open(&options.receiver)?;
    crate::review_from_store(&store, &options.query, &options.selection, &options.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Vec<String> {
        [
            "--endpoint",
            "http://127.0.0.1:8765",
            "--token-file",
            "/private/token",
            "--harness-id",
            "harness.codex",
            "--session-id",
            "exact-session",
            "--turn-id",
            "exact-turn",
            "--linked-traces",
            "--format",
            "json",
            "--limit",
            "8",
        ]
        .map(str::to_owned)
        .into()
    }

    #[test]
    fn remote_review_preserves_exact_scope_without_opening_a_local_history() {
        let Command::RemoteReview(options) = parse(&args()).unwrap() else {
            panic!("remote receipt");
        };
        assert_eq!(options.query.session_id.as_deref(), Some("exact-session"));
        assert_eq!(options.query.limit, 8);
        assert!(options.selection.linked_traces);
        assert!(
            matches!(options.selection.anchor, crate::ReviewAnchor::Turn { ref turn_id, worker_id: None } if turn_id == "exact-turn")
        );
        assert_eq!(
            options.output.format,
            Some(crate::review_receipt::ReviewFormat::Json)
        );
    }

    #[test]
    fn remote_review_rejects_ambiguous_local_and_legacy_receivers() {
        for extra in [
            ["--db", "history.sqlite"],
            ["--receiver-protocol", "python-v0.2.8"],
            ["--endpoint", "http://127.0.0.1:8766"],
        ] {
            let mut args = args();
            args.extend(extra.map(str::to_owned));
            assert!(parse(&args).is_err());
        }
        let mut args = args();
        args[1] = "http://192.0.2.1:8765".into();
        assert!(parse(&args).is_err());
    }

    #[test]
    fn tagged_review_requires_an_explicit_closed_scope_and_export_type() {
        let base = [
            "--receiver-protocol",
            "python-v0.2.8",
            "--endpoint",
            "http://127.0.0.1:8765",
            "--token-file",
            "fixture-token",
            "--service",
            "opencode",
            "--agent-id",
            "worker/1",
            "--session-id",
            "session?not-query",
            "--turn-of-trace",
            "0123456789abcdef0123456789abcdef",
            "-o",
            "capture.json",
        ]
        .map(str::to_owned);
        let Command::LegacyReview(options) = parse(&base).unwrap() else {
            panic!("tagged receiver Review");
        };
        assert!(options.whole_turn);
        assert!(options.receiver.is_python());
        assert_eq!(options.pages, 1);
        assert_eq!(options.format, ReviewFormat::Text);
        assert_eq!(options.session_id, "session?not-query");
        for extra in [
            ["--db", "local.sqlite"],
            ["--pages", "6"],
            ["--service", "codex"],
            ["--trace-id", "abcdef0123456789abcdef0123456789"],
            ["--output", "other.json"],
        ] {
            let mut invalid = base.to_vec();
            invalid.extend(extra.map(str::to_owned));
            assert!(parse(&invalid).is_err());
        }
        let mut unsupported = base.clone();
        unsupported[15] = "capture.exe".into();
        assert!(parse(&unsupported).is_err());
        let mut untagged = base.clone();
        untagged[1] = "python-v0.2.7".into();
        assert!(parse(&untagged).is_err());
    }
}
