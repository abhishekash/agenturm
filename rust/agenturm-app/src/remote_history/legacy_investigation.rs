//! Bounded typed investigations over an explicitly selected Python receiver.

mod reference;

use super::legacy::LegacyReceiver;
use super::legacy_review::{CapturePolicy, LocalPolicy, encode_query};
use super::{RemoteHistoryOptions, parse_attach};
use reference::{Reference, SourceResponse, safe_label, safe_text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::time::{Duration, Instant};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_TERM: usize = 4096;
const MAX_PAGE: usize = 100;
const MAX_PAGES: usize = 5;
const FORMAT: &str = "agenturm-investigation-v1";
const POLICY: &str = "Python receiver capture policy changed or forbids investigation evidence";
const USAGE: &str = "Supply investigate <path|command|instruction|source> <selection> --receiver-protocol python-v0.2.8 --endpoint <loopback-http-url> --token-file <path>";
const COVERAGE: &str = "Retained indexed source observations only. Clipped, uncaptured, unsupported or pruned evidence can be absent. Recorded file attempts do not prove a persisted change.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Options {
    receiver: RemoteHistoryOptions,
    selection: Selection,
    json: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Selection {
    Search(Search),
    Source(Box<Reference>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Search {
    kind: Kind,
    query: String,
    service: Option<String>,
    failed: bool,
    limit: usize,
    pages: usize,
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Path,
    Command,
    Instruction,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Command => "command",
            Self::Instruction => "instruction",
        }
    }

    fn match_mode(self) -> &'static str {
        if self == Self::Path {
            "exact recorded path; case and Unicode sensitive"
        } else {
            "literal word phrase; case insensitive, accents preserved"
        }
    }
}

pub(crate) fn parse(args: &[String]) -> Result<Options, String> {
    let [verb, query, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    let mut flags = BTreeMap::new();
    let mut args = rest.iter();
    let mut failed = false;
    while let Some(flag) = args.next() {
        if flag == "--failed" && !failed {
            failed = true;
            continue;
        }
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or(USAGE)?;
        if !flag.starts_with("--") || flags.insert(flag.as_str(), value.as_str()).is_some() {
            return Err(USAGE.into());
        }
    }
    let mut receiver = Vec::new();
    for flag in ["--endpoint", "--token-file", "--receiver-protocol"] {
        let value = flags.remove(flag).ok_or(USAGE)?;
        if !safe_label(value, 4096) {
            return Err(USAGE.into());
        }
        receiver.extend([flag.to_owned(), value.to_owned()]);
    }
    let receiver = parse_attach(&receiver)?;
    if !receiver.is_python() {
        return Err(USAGE.into());
    }
    let json = match flags.remove("--format") {
        Some("json") => true,
        Some("text") | None => false,
        _ => return Err("Investigation format must be text or json".into()),
    };
    let selection = if verb == "source" {
        if failed || !safe_label(query, 512) {
            return Err("Supply one exact investigation event ID and its source reference".into());
        }
        let reference = Reference::decode(
            flags
                .remove("--reference")
                .ok_or("Source reads require --reference from an investigation result")?,
        )?;
        if reference.event_id != *query {
            return Err("Source reference differs from the selected event".into());
        }
        Selection::Source(Box::new(reference))
    } else {
        let kind = match verb.as_str() {
            "path" => Kind::Path,
            "command" => Kind::Command,
            "instruction" => Kind::Instruction,
            _ => return Err(USAGE.into()),
        };
        if !safe_label(query, MAX_TERM)
            || query.trim().is_empty()
            || (kind != Kind::Path && !query.chars().any(char::is_alphanumeric))
        {
            return Err("Supply a bounded single-line investigation query".into());
        }
        let service = flags.remove("--service").map(str::to_owned);
        let cursor = flags.remove("--cursor").map(str::to_owned);
        if service
            .as_deref()
            .is_some_and(|service| !safe_label(service, 128))
            || cursor
                .as_deref()
                .is_some_and(|cursor| !safe_label(cursor, MAX_TERM))
        {
            return Err("Invalid investigation service or opaque cursor".into());
        }
        Selection::Search(Search {
            kind,
            query: query.clone(),
            failed,
            service,
            cursor,
            limit: number(flags.remove("--limit"), 50, MAX_PAGE)?,
            pages: number(flags.remove("--pages"), 1, MAX_PAGES)?,
        })
    };
    if !flags.is_empty() {
        return Err("Unexpected option for the selected Python investigation".into());
    }
    Ok(Options {
        receiver,
        selection,
        json,
    })
}

fn number(value: Option<&str>, default: usize, max: usize) -> Result<usize, String> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|number| (1..=max).contains(number))
                .ok_or_else(|| format!("Investigation value must be between 1 and {max}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[derive(Deserialize)]
struct Health {
    ok: bool,
    receiver_id: String,
    #[serde(default)]
    investigation_index_v1: bool,
    indexing: Indexing,
    security: Security,
    #[serde(default)]
    local_agents: LocalPolicy,
}

#[derive(Deserialize)]
struct Indexing {
    ready: bool,
}

#[derive(Deserialize)]
struct Security {
    authentication_required: bool,
    host_validation: bool,
}

fn health(receiver: &LegacyReceiver, deadline: Instant) -> Result<LocalPolicy, String> {
    let health: Health = receiver.get_for_review("/api/health", deadline)?;
    if !health.ok
        || health.receiver_id != receiver.native_identity()
        || !health.security.authentication_required
        || !health.security.host_validation
    {
        return Err("Python receiver investigation identity or authentication changed".into());
    }
    if !health.indexing.ready || !health.investigation_index_v1 {
        return Err(
            "Python receiver typed investigation index is unavailable or catching up".into(),
        );
    }
    if !health.local_agents.investigation_ready() {
        return Err(POLICY.into());
    }
    Ok(health.local_agents)
}

#[derive(Deserialize)]
struct Page {
    format: String,
    receiver_id: String,
    kind: Kind,
    query: String,
    matches: Vec<Match>,
    next_cursor: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Match {
    event_id: String,
    text: String,
    time_unix_nano: u64,
    result: String,
    action: Option<String>,
    term_complete: bool,
    source_reference: Reference,
    #[serde(skip_deserializing)]
    source_command: String,
}

#[derive(Serialize)]
struct Report<'a> {
    format: &'static str,
    receiver_id: &'a str,
    kind: Kind,
    query: &'a str,
    match_mode: &'static str,
    scope: &'static str,
    coverage: &'static str,
    matches: Vec<Match>,
    next_cursor: Option<String>,
    pages_read: usize,
    limited: bool,
    payload_limit_bytes: usize,
}

struct PreparedOutput {
    body: Vec<u8>,
    witnesses: Vec<(Reference, CapturePolicy)>,
}

impl Search {
    fn path(&self, cursor: Option<&str>) -> String {
        format!(
            "/api/investigations?kind={}&q={}&limit={}&service={}&failed={}&cursor={}",
            self.kind.name(),
            encode_query(&self.query),
            self.limit,
            encode_query(self.service.as_deref().unwrap_or_default()),
            u8::from(self.failed),
            encode_query(cursor.unwrap_or_default())
        )
    }

    fn check_page(&self, page: &Page, receiver: &str) -> Result<(), String> {
        if page.format != FORMAT
            || page.receiver_id != receiver
            || page.kind != self.kind
            || page.query != self.query
            || page.matches.len() > self.limit
            || page
                .next_cursor
                .as_deref()
                .is_some_and(|cursor| !safe_label(cursor, MAX_TERM))
            || (page.matches.is_empty() && page.next_cursor.is_some())
        {
            return Err(
                "Python receiver investigation page differs from the requested query".into(),
            );
        }
        Ok(())
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn output(report: &Report<'_>, json: bool) -> Result<Vec<u8>, String> {
    if json {
        return serde_json::to_vec(report)
            .map_err(|_| "Cannot encode investigation results".into());
    }
    let mut output = format!(
        "{} retained {} observation(s) · {}\n",
        report.matches.len(),
        report.kind.name(),
        report.match_mode
    );
    for row in &report.matches {
        use std::fmt::Write as _;
        let [service, agent, session, ..] = row.source_reference.owner_fields();
        let owner = [service, agent, session].map(|part| part.unwrap_or("Not reported"));
        let _ = write!(
            output,
            "\n{}\n{} / {} / {}\nRecorded result: {}\nExact source: {}\n",
            row.text, owner[0], owner[1], owner[2], row.result, row.source_command
        );
        if !row.term_complete {
            output.push_str("Only bounded or partial input was available.\n");
        }
    }
    output.push('\n');
    output.push_str(COVERAGE);
    if let Some(cursor) = &report.next_cursor {
        output.push_str("\nMore observations remain. Resume with --cursor ");
        output.push_str(&quote(cursor));
    }
    Ok(output.into_bytes())
}

fn read_search(
    receiver: &LegacyReceiver,
    options: &Options,
    search: &Search,
    initial: &LocalPolicy,
    deadline: Instant,
) -> Result<PreparedOutput, String> {
    let mut report = Report {
        format: FORMAT,
        receiver_id: receiver.native_identity(),
        kind: search.kind,
        query: &search.query,
        match_mode: search.kind.match_mode(),
        scope: if search.service.is_some() {
            "explicit Activity ownership filter"
        } else {
            "global retained Activity history"
        },
        coverage: COVERAGE,
        matches: Vec::new(),
        next_cursor: search.cursor.clone(),
        pages_read: 0,
        limited: false,
        payload_limit_bytes: MAX_BYTES,
    };
    let mut seen_events = BTreeSet::new();
    let mut seen_cursors: BTreeSet<_> = search.cursor.iter().cloned().collect();
    let mut used = 0;
    let mut encoded = Vec::new();
    let mut witnesses = Vec::new();
    for _ in 0..search.pages {
        let Some((mut page, size)) = receiver.get_for_investigation::<Page>(
            &search.path(report.next_cursor.as_deref()),
            deadline,
            MAX_BYTES - used,
        )?
        else {
            break;
        };
        used += size;
        search.check_page(&page, receiver.native_identity())?;
        if let Some(cursor) = &page.next_cursor
            && !seen_cursors.insert(cursor.clone())
        {
            return Err("Python receiver repeated an opaque investigation cursor".into());
        }
        let mut page_witnesses = Vec::with_capacity(page.matches.len());
        for row in &mut page.matches {
            let reference = &row.source_reference;
            if reference.receiver_id != receiver.native_identity()
                || reference.event_id != row.event_id
                || !seen_events.insert(row.event_id.clone())
                || search
                    .service
                    .as_deref()
                    .is_some_and(|service| reference.service.as_deref() != Some(service))
                || !safe_text(&row.text, MAX_TERM)
                || !safe_label(&row.result, 128)
                || row
                    .action
                    .as_deref()
                    .is_some_and(|action| !safe_label(action, 128))
                || (search.failed
                    && !matches!(
                        row.result.as_str(),
                        "error"
                            | "failed"
                            | "failure"
                            | "denied"
                            | "blocked"
                            | "policy_denied"
                            | "incomplete"
                    ))
            {
                return Err(
                    "Python receiver investigation event, scope or result could not be verified"
                        .into(),
                );
            }
            page_witnesses.push((reference.clone(), reference.policy(initial)?));
            row.source_command = format!(
                "agenturm investigate source {} --reference {} --receiver-protocol python-v0.2.8 --endpoint {} --token-file {}",
                quote(&row.event_id),
                quote(&reference.encode()?),
                quote(&format!("http://{}", options.receiver.endpoint())),
                quote(
                    options
                        .receiver
                        .token_file()
                        .and_then(|path| path.to_str())
                        .ok_or(USAGE)?,
                )
            );
        }
        report.matches.extend(page.matches);
        report.next_cursor = page.next_cursor;
        report.pages_read += 1;
        report.limited = report.next_cursor.is_some();
        let trial = output(&report, options.json)?;
        if trial.len() >= MAX_BYTES {
            break;
        }
        encoded = trial;
        witnesses.extend(page_witnesses);
        if !report.limited {
            break;
        }
    }
    if encoded.is_empty() {
        return Err("First investigation page exceeds the 2 MiB budget; lower --limit".into());
    }
    Ok(PreparedOutput {
        body: encoded,
        witnesses,
    })
}

fn read_source(
    receiver: &LegacyReceiver,
    reference: &Reference,
    initial: &LocalPolicy,
    deadline: Instant,
) -> Result<PreparedOutput, String> {
    if reference.receiver_id != receiver.native_identity() {
        return Err("Investigation source belongs to a different Python receiver".into());
    }
    let policy = reference.policy(initial)?;
    let url = format!(
        "/api/investigations/source?event_id={}&receiver_id={}&reference={}",
        encode_query(&reference.event_id),
        encode_query(&reference.receiver_id),
        encode_query(&reference.encode()?)
    );
    let Some((data, _)) =
        receiver.get_for_investigation::<SourceResponse>(&url, deadline, MAX_SOURCE_BYTES)?
    else {
        return Err("Exact investigation source exceeds its 64 KiB ceiling".into());
    };
    data.verify(reference, initial)?;
    let encoded =
        serde_json::to_vec(&data).map_err(|_| "Cannot encode exact investigation source")?;
    if encoded.len() >= MAX_SOURCE_BYTES {
        return Err("Exact investigation source exceeds its 64 KiB ceiling".into());
    }
    Ok(PreparedOutput {
        body: encoded,
        witnesses: vec![(reference.clone(), policy)],
    })
}

pub(crate) fn execute(options: &Options, mut stdout: impl Write) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let receiver = LegacyReceiver::connect(&options.receiver)?;
    let initial = health(&receiver, deadline)?;
    let PreparedOutput {
        mut body,
        witnesses,
    } = match &options.selection {
        Selection::Search(search) => read_search(&receiver, options, search, &initial, deadline)?,
        Selection::Source(reference) => read_source(&receiver, reference, &initial, deadline)?,
    };
    body.push(b'\n');
    let current = health(&receiver, deadline)?;
    for (reference, policy) in witnesses {
        if reference.policy(&current)? != policy {
            return Err(POLICY.into());
        }
    }
    match stdout.write_all(&body).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(_) => Err("Cannot publish investigation evidence".into()),
    }
}
