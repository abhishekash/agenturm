//! Bounded import and exact display of agent-evals results.json metadata.

use agenturm_kernel::ExtensionId;
use agenturm_store::{
    EvalDetailRecord, EvalImportReport, EvalInventory, EvalInventoryQuery, EvalOutcome, EvalPage,
    EvalQuery, EvalResultDetail, EvalScore, EvalScoreDetail, MAX_EVAL_INVENTORY_SOURCES,
    MAX_EVAL_OUTCOMES, RetainedEvalOutcome, Store,
};
use serde::Deserialize;
use serde_json::Number;
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) const IMPORT_USAGE: &str = "agenturm import-evals --file <results.json|-> --db <path> --source-id otlp.<id> [--include-detail]";
pub(super) const QUERY_USAGE: &str = concat!(
    "agenturm evals --db <path> --source-id otlp.<id> [--trace-id <id> [--history [--before-revision <n>]] | --summary [--after-trace-id <id>]] [--limit <1..256>] [--json]\n",
    "agenturm evals --list --db <path> --source-id otlp.<id> [--source-id otlp.<id> ...] [--json]",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvalOptions {
    db: PathBuf,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Import {
        source_id: ExtensionId,
        path: PathBuf,
        include_detail: bool,
    },
    Read {
        source_id: ExtensionId,
        trace_id: String,
        json: bool,
    },
    Browse {
        source_id: ExtensionId,
        query: EvalQuery,
        json: bool,
    },
    List {
        query: EvalInventoryQuery,
        json: bool,
    },
}

pub(super) fn parse(arguments: &[String], import: bool) -> Result<EvalOptions, String> {
    let usage = if import { IMPORT_USAGE } else { QUERY_USAGE };
    let (mut db, mut value) = (None, None);
    let mut source_ids = Vec::new();
    let (mut json, mut history, mut summary, mut list) = (false, false, false, false);
    let mut include_detail = false;
    let (mut limit, mut before, mut after) = (None, None, None);
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        let boolean = match (flag.as_str(), import) {
            ("--include-detail", true) => Some(&mut include_detail),
            ("--json", false) => Some(&mut json),
            ("--history", false) => Some(&mut history),
            ("--summary", false) => Some(&mut summary),
            ("--list", false) => Some(&mut list),
            _ => None,
        };
        if let Some(boolean) = boolean {
            if *boolean {
                return Err(usage.into());
            }
            *boolean = true;
            continue;
        }
        let argument = arguments
            .next()
            .filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
            .ok_or(usage)?;
        let slot = match flag.as_str() {
            "--db" if argument != "-" && argument != ":memory:" => &mut db,
            "--source-id" if source_ids.len() < MAX_EVAL_INVENTORY_SOURCES => {
                source_ids.push(crate::trace_import::parse_source_id(argument)?);
                continue;
            }
            "--file" if import => &mut value,
            "--trace-id" if !import => &mut value,
            "--limit" if !import => &mut limit,
            "--before-revision" if !import => &mut before,
            "--after-trace-id" if !import => &mut after,
            _ => return Err(usage.into()),
        };
        if slot.replace(argument).is_some() {
            return Err(usage.into());
        }
    }
    let db = PathBuf::from(db.ok_or(usage)?);
    if list {
        if history
            || summary
            || value.is_some()
            || limit.is_some()
            || before.is_some()
            || after.is_some()
        {
            return Err(usage.into());
        }
        let query = EvalInventoryQuery { source_ids };
        query.validate().map_err(|_| usage)?;
        return Ok(EvalOptions {
            db,
            action: Action::List { query, json },
        });
    }
    let [source_id] = source_ids.try_into().map_err(|_| usage)?;
    let action = if import {
        Action::Import {
            source_id,
            path: PathBuf::from(value.ok_or(usage)?),
            include_detail,
        }
    } else if history || summary {
        if history && summary {
            return Err(usage.into());
        }
        let limit = limit
            .map(|value| value.parse::<usize>().map_err(|_| usage))
            .transpose()?
            .unwrap_or(64);
        if !(1..=256).contains(&limit) {
            return Err(usage.into());
        }
        let query = if history {
            if after.is_some() {
                return Err(usage.into());
            }
            let before_revision = before
                .map(|value| value.parse::<u64>().map_err(|_| usage))
                .transpose()?;
            if before_revision.is_some_and(|n| n == 0 || n > i64::MAX as u64) {
                return Err(usage.into());
            }
            EvalQuery::History {
                trace_id: value.ok_or(usage)?.to_ascii_lowercase(),
                before_revision,
                limit,
            }
        } else {
            if value.is_some() || before.is_some() {
                return Err(usage.into());
            }
            EvalQuery::Latest {
                after_trace_id: after.map(|value| value.to_ascii_lowercase()),
                limit,
            }
        };
        Action::Browse {
            source_id,
            query,
            json,
        }
    } else {
        if limit.is_some() || before.is_some() || after.is_some() {
            return Err(usage.into());
        }
        Action::Read {
            source_id,
            trace_id: value.ok_or(usage)?.to_ascii_lowercase(),
            json,
        }
    };
    Ok(EvalOptions { db, action })
}

pub fn execute_evals(options: &EvalOptions) -> Result<String, String> {
    match &options.action {
        Action::Import {
            source_id,
            path,
            include_detail,
        } => {
            let Prepared { outcomes, details } = prepare(path, *include_detail)?;
            if outcomes.is_empty() {
                return Ok(report(EvalImportReport::default()));
            }
            // Evaluation records are attached to existing history only.
            Store::open_read_only(&options.db).map_err(history_error)?;
            let mut store = Store::open_collector(&options.db).map_err(history_error)?;
            let imported = store
                .import_eval_outcomes_with_details(source_id, &outcomes, &details)
                .map_err(|error| error.to_string())?;
            Ok(report(imported))
        }
        Action::Browse {
            source_id,
            query,
            json,
        } => {
            let store = Store::open_read_only(&options.db).map_err(history_error)?;
            let page = store
                .query_eval_outcomes(source_id, query)
                .map_err(|error| error.to_string())?;
            display_page(page, query, *json)
        }
        Action::Read {
            source_id,
            trace_id,
            json,
        } => {
            let store = Store::open_read_only(&options.db).map_err(history_error)?;
            let outcome = store
                .eval_outcome(source_id, trace_id)
                .map_err(|error| error.to_string())?;
            display(outcome.as_ref(), *json)
        }
        Action::List { query, json } => {
            let store = Store::open_read_only(&options.db).map_err(history_error)?;
            let inventory = store
                .query_eval_inventory(query)
                .map_err(|error| error.to_string())?;
            display_inventory(&inventory, *json)
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &EvalOptions,
    store: &agenturm_storage_service::NativeStore,
) -> Result<String, String> {
    match &options.action {
        Action::Import {
            source_id,
            path,
            include_detail,
        } => {
            let Prepared { outcomes, details } = prepare(path, *include_detail)?;
            let imported = if details.is_empty() {
                store.import_eval_outcomes(source_id, &outcomes)
            } else {
                store.import_eval_outcomes_with_details(source_id, &outcomes, &details)
            }
            .map_err(|error| error.to_string())?;
            Ok(report(imported))
        }
        Action::Browse {
            source_id,
            query,
            json,
        } => {
            let page = store
                .query_eval_outcomes(source_id, query)
                .map_err(|error| error.to_string())?;
            display_page(page, query, *json)
        }
        Action::Read {
            source_id,
            trace_id,
            json,
        } => {
            let outcome = store
                .eval_outcome(source_id, trace_id)
                .map_err(|error| error.to_string())?;
            display(outcome.as_ref(), *json)
        }
        Action::List { query, json } => {
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;
            let store = store
                .clone()
                .with_workspace_identity(&identity)
                .map_err(|error| error.to_string())?;
            let inventory = store
                .query_eval_inventory(query)
                .map_err(|error| error.to_string())?;
            display_inventory(&inventory, *json)
        }
    }
}

fn history_error(error: agenturm_store::StoreError) -> String {
    format!("open history failed: {error}")
}

struct Prepared {
    outcomes: Vec<EvalOutcome>,
    details: Vec<EvalDetailRecord>,
}

fn prepare(path: &Path, include_detail: bool) -> Result<Prepared, String> {
    use crate::trace_import::{MAX_EXPORT_BYTES, open_regular_export};

    let mut bytes = Vec::new();
    if path.as_os_str() == "-" {
        io::stdin()
            .lock()
            .take(MAX_EXPORT_BYTES + 1)
            .read_to_end(&mut bytes)
    } else {
        open_regular_export(path, "evaluation results")?
            .take(MAX_EXPORT_BYTES + 1)
            .read_to_end(&mut bytes)
    }
    .map_err(|_| "Cannot read evaluation results")?;
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("Evaluation results exceed 4 MiB".into());
    }
    let file: Results = serde_json::from_slice(&bytes).map_err(decode_error)?;
    if file.results.len() > MAX_EVAL_OUTCOMES {
        return Err("Evaluation results exceed 1024 outcomes".into());
    }
    let outcomes = file
        .results
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let outcome = record.into_outcome();
            outcome
                .validate()
                .map_err(|_| format!("Invalid evaluation metadata in result {}", index + 1))?;
            Ok(outcome)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let details = if include_detail {
        let detail_file: DetailResults = serde_json::from_slice(&bytes).map_err(decode_error)?;
        detail_file
            .results
            .into_iter()
            .zip(&outcomes)
            .filter_map(|(row, outcome)| row.into_record(&outcome.trace_id))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Prepared { outcomes, details })
}

fn decode_error(error: serde_json::Error) -> String {
    format!(
        "Invalid evaluation results at line {} column {}",
        error.line(),
        error.column()
    )
}

#[derive(Deserialize)]
struct Results {
    results: Vec<ResultRow>,
}

#[derive(Deserialize)]
struct ResultRow {
    trace_id: String,
    passed: bool,
    task_id: Option<String>,
    provider_model: Option<String>,
    scores: Option<Vec<ScoreRow>>,
}

#[derive(Deserialize)]
struct ScoreRow {
    scorer: String,
    passed: Option<bool>,
    score: Option<Number>,
}

#[derive(Deserialize)]
struct DetailResults {
    results: Vec<DetailRow>,
}

#[derive(Deserialize)]
struct DetailRow {
    error: Option<serde_json::Value>,
    scores: Option<Vec<DetailScore>>,
}

#[derive(Deserialize)]
struct DetailScore {
    detail: Option<serde_json::Value>,
}

impl DetailRow {
    fn into_record(self, trace_id: &str) -> Option<EvalDetailRecord> {
        let text = |value: serde_json::Value| match value {
            serde_json::Value::String(text) if !text.is_empty() => Some(text),
            _ => None,
        };
        let error = self.error.and_then(text);
        let scores: Vec<_> = self
            .scores
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .filter_map(|(score_index, score)| {
                score.detail.and_then(text).map(|detail| EvalScoreDetail {
                    score_index,
                    detail,
                })
            })
            .collect();
        (error.is_some() || !scores.is_empty()).then(|| EvalDetailRecord {
            trace_id: trace_id.into(),
            detail: EvalResultDetail { error, scores },
        })
    }
}

impl ResultRow {
    fn into_outcome(self) -> EvalOutcome {
        EvalOutcome {
            trace_id: self.trace_id.to_ascii_lowercase(),
            passed: self.passed,
            task_id: self.task_id.filter(|value| !value.is_empty()),
            provider_model: self.provider_model.filter(|value| !value.is_empty()),
            scores: self
                .scores
                .unwrap_or_default()
                .into_iter()
                .map(|score| EvalScore {
                    scorer: score.scorer,
                    passed: score.passed,
                    score: score.score,
                })
                .collect(),
        }
    }
}

fn report(report: EvalImportReport) -> String {
    format!(
        "Imported {} evaluations. Corrected {}. Replayed {}.\n",
        report.inserted, report.corrected, report.replayed,
    )
}

fn display(evaluation: Option<&RetainedEvalOutcome>, json: bool) -> Result<String, String> {
    if json {
        return serde_json::to_string(&evaluation)
            .map(|value| value + "\n")
            .map_err(|_| "Cannot encode evaluation metadata".into());
    }
    let Some(evaluation) = evaluation else {
        return Ok("No retained evaluation for this trace.\n".into());
    };
    Ok(display_outcome(
        &evaluation.trace_source_id,
        evaluation.revision,
        &evaluation.outcome,
    ))
}

fn display_outcome(source: &ExtensionId, revision: u64, outcome: &EvalOutcome) -> String {
    let status = if outcome.passed { "passed" } else { "failed" };
    let mut output = format!(
        "Evaluation {status}\nTrace {} {}\nRevision {}\n",
        source.as_str(),
        outcome.trace_id,
        revision,
    );
    if let Some(task) = &outcome.task_id {
        let _ = writeln!(&mut output, "Task {task}");
    }
    if let Some(provider) = &outcome.provider_model {
        let _ = writeln!(&mut output, "Provider model {provider}");
    }
    for score in &outcome.scores {
        let _ = write!(&mut output, "Scorer {}", score.scorer);
        if let Some(passed) = score.passed {
            output.push_str(if passed { " passed" } else { " failed" });
        }
        if let Some(value) = &score.score {
            let _ = write!(&mut output, " score {value}");
        }
        output.push('\n');
    }
    output
}

fn display_inventory(inventory: &EvalInventory, json: bool) -> Result<String, String> {
    if json {
        return serde_json::to_string(inventory)
            .map(|text| text + "\n")
            .map_err(|_| "Cannot encode evaluation inventory".into());
    }
    let sources = inventory
        .source_ids
        .iter()
        .map(ExtensionId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "Current evaluation metadata\nSources {sources}\nTrace evidence snapshot {}\nRetained outcomes {}\n",
        inventory.trace_evidence_snapshot,
        inventory.outcomes.len()
    );
    for row in &inventory.outcomes {
        text.push('\n');
        text.push_str(&display_outcome(
            &row.trace_source_id,
            row.revision,
            &row.outcome,
        ));
    }
    Ok(text)
}

fn display_page(page: EvalPage, query: &EvalQuery, json: bool) -> Result<String, String> {
    let passed = page
        .outcomes
        .iter()
        .filter(|row| row.outcome.passed)
        .count();
    let failed = page.outcomes.len() - passed;
    let view = if matches!(query, EvalQuery::History { .. }) {
        "revisions"
    } else {
        "current outcomes"
    };
    if json {
        return serde_json::to_string(&serde_json::json!({ "view": view, "coverage": "current page", "passed": passed, "failed": failed, "page": page }))
            .map(|text| text + "\n").map_err(|_| "Cannot encode evaluation page".into());
    }
    let mut text = format!("Evaluation {view}: {passed} passed, {failed} failed on this page\n");
    for outcome in &page.outcomes {
        text.push_str(&display(Some(outcome), false)?);
    }
    if let Some(cursor) = page.next_trace_id {
        let _ = writeln!(text, "More outcomes: --after-trace-id {cursor}");
    }
    if let Some(cursor) = page.next_before_revision {
        let _ = writeln!(text, "More revisions: --before-revision {cursor}");
    }
    Ok(text)
}

pub(super) fn workspace_evals(
    store: &impl crate::HistoryReader,
    request: agenturm_ui::EvaluationRead,
) -> Result<agenturm_ui::WorkResult, String> {
    use agenturm_ui::{
        EvaluationDetailEntry, EvaluationPage, EvaluationRow, EvaluationView, WorkResult,
    };
    const PAGE_SIZE: usize = 32;
    let view = match &request.view {
        EvaluationView::Revisions { before } => EvalQuery::History {
            trace_id: request.trace.trace_id.clone(),
            before_revision: *before,
            limit: PAGE_SIZE,
        },
        EvaluationView::SourceSummary { after } => EvalQuery::Latest {
            after_trace_id: after.clone(),
            limit: PAGE_SIZE,
        },
    };
    let query = agenturm_store::SelectedEvalQuery {
        trace_id: request.trace.trace_id.clone(),
        evidence_snapshot: request.trace.snapshot_id,
        view,
    };
    let selected = store
        .query_selected_evals(&request.trace.source_id, &query)?
        .ok_or("The selected trace is no longer available. Reopen Traces to refresh.")?;
    let continuation = match &request.view {
        EvaluationView::Revisions { .. } => {
            selected
                .page
                .next_before_revision
                .map(|before| EvaluationView::Revisions {
                    before: Some(before),
                })
        }
        EvaluationView::SourceSummary { .. } => selected
            .page
            .next_trace_id
            .map(|after| EvaluationView::SourceSummary { after: Some(after) }),
    };
    let rows = selected
        .page
        .outcomes
        .into_iter()
        .map(|row| {
            let mut details = vec![
                format!(
                    "Task {}",
                    row.outcome.task_id.as_deref().unwrap_or("unknown")
                ),
                format!(
                    "Model {}",
                    row.outcome.provider_model.as_deref().unwrap_or("unknown")
                ),
            ];
            details.extend(row.outcome.scores.iter().map(|score| {
                let verdict = match score.passed {
                    Some(true) => "passed",
                    Some(false) => "failed",
                    None => "verdict unknown",
                };
                let value = score
                    .score
                    .as_ref()
                    .map_or_else(|| "score unknown".into(), |score| format!("score {score}"));
                format!("{} · {verdict} · {value}", score.scorer)
            }));
            EvaluationRow {
                trace_id: row.outcome.trace_id,
                revision: row.revision,
                passed: row.outcome.passed,
                details,
                private_detail: row
                    .detail_selection
                    .filter(|_| store.allows_private_capture())
                    .map(|detail| EvaluationDetailEntry {
                        source: detail.trace_source_id,
                        trace_id: detail.trace_id,
                        revision: detail.revision,
                        receipt_id: detail.receipt_id,
                        snapshot_id: detail.snapshot_id,
                    }),
            }
        })
        .collect();
    Ok(WorkResult::Evaluations(Box::new(EvaluationPage {
        request,
        current_revision: selected.current_revision,
        rows,
        continuation,
    })))
}
