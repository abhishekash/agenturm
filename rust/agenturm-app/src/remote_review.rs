//! Bounded Review receipts through an explicitly selected native receiver.

use crate::remote_history::{self, RemoteHistoryOptions, RemoteStore};
use crate::{Command, HistoryQueryOptions, ReviewOutput, ReviewSelection};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteReviewOptions {
    receiver: RemoteHistoryOptions,
    query: HistoryQueryOptions,
    selection: ReviewSelection,
    output: ReviewOutput,
}

pub(crate) fn parse(args: &[String]) -> Result<Command, String> {
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
}
