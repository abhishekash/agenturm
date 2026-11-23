//! Bounded one-shot cockpit evidence without entering the terminal UI.

use crate::{Command, HistoryReader, Store, cockpit_startup};
use agenturm_store::{SessionPage, SessionQuery};
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SummaryOptions {
    db: Option<PathBuf>,
    json: bool,
    limit: usize,
    #[cfg(target_os = "linux")]
    remote: Option<crate::RemoteHistoryOptions>,
}

pub(super) fn parse(args: &[String]) -> Result<SummaryOptions, String> {
    let mut filtered = vec!["cockpit".to_owned()];
    let mut once = false;
    let mut json = false;
    let mut limit = None;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--once" if !once => once = true,
            "--json" if !json => json = true,
            "--limit" if limit.is_none() => {
                index += 1;
                limit = Some(
                    args.get(index)
                        .and_then(|arg| arg.parse::<usize>().ok())
                        .filter(|limit| (1..=agenturm_store::MAX_SESSION_PAGE_SIZE).contains(limit))
                        .ok_or("Summary limit must be 1..256")?,
                );
            }
            "--once" | "--json" | "--limit" => return Err("Duplicate summary option".into()),
            _ => filtered.push(arg.clone()),
        }
        index += 1;
    }
    if !once {
        return Err("Supply --once for a cockpit summary".into());
    }
    let mut result = SummaryOptions {
        db: None,
        json,
        limit: limit.unwrap_or(64),
        #[cfg(target_os = "linux")]
        remote: None,
    };
    match crate::parse_command(&filtered)? {
        Command::Cockpit { db, extensions } if extensions.extensions.is_empty() => result.db = db,
        #[cfg(target_os = "linux")]
        Command::RemoteCockpit(remote) => result.remote = Some(remote),
        _ => {
            return Err(
                "One-shot summaries use retained session evidence; omit UI extension options"
                    .into(),
            );
        }
    }
    Ok(result)
}

pub fn execute_summary(options: &SummaryOptions) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    if let Some(remote) = &options.remote {
        let store = crate::remote_history::RemoteStore::open(remote)?;
        return from_reader(&store, options, None);
    }
    let startup = if options.db.is_none() {
        Some(cockpit_startup::prepare_cockpit_store(None)?)
    } else {
        None
    };
    let db = options
        .db
        .as_ref()
        .or_else(|| startup.as_ref().map(|startup| &startup.db))
        .ok_or("Summary history is unavailable")?;
    let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
    from_reader(
        &store,
        options,
        startup.as_ref().and_then(|startup| startup.notice()),
    )
}

pub(super) fn from_reader(
    store: &impl HistoryReader,
    options: &SummaryOptions,
    notice: Option<String>,
) -> Result<String, String> {
    let page = store.query_sessions(&SessionQuery::latest(options.limit))?;
    render(page, options.json, notice)
}

fn render(page: SessionPage, json: bool, notice: Option<String>) -> Result<String, String> {
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "format": "agenturm-summary/1", "snapshot_id": page.snapshot_id.to_string(),
            "sessions": page.sessions, "next_before": page.next_before,
            "has_more": page.next_before.is_some(), "notice": notice,
        }))
        .map(|text| text + "\n")
        .map_err(|_| "Cannot encode cockpit summary".into());
    }
    let mut output = format!(
        "Agenturm · {} retained session(s) · snapshot {}\n",
        page.sessions.len(),
        page.snapshot_id
    );
    for session in page.sessions {
        let state = session
            .lifecycle
            .as_ref()
            .map(|lifecycle| lifecycle.state.as_str())
            .unwrap_or("unknown");
        writeln!(
            &mut output,
            "{} / {} · {} · {} retained observation(s)",
            session.harness_id.as_str(),
            session.session_id.escape_default(),
            state,
            session.review_version.retained_observations
        )
        .map_err(|_| "Cannot render cockpit summary")?;
    }
    if page.next_before.is_some() {
        output.push_str(
            "Older sessions are outside this page. Use query or the cockpit to continue.\n",
        );
    }
    if let Some(notice) = notice {
        writeln!(&mut output, "{}", notice.escape_default())
            .map_err(|_| "Cannot render summary notice")?;
    }
    Ok(output)
}
