//! Bounded import and exact display of agent-evals results.json metadata.

use agenturm_kernel::ExtensionId;
use agenturm_store::{
    EvalImportReport, EvalOutcome, EvalPage, EvalQuery, EvalScore, MAX_EVAL_OUTCOMES,
    RetainedEvalOutcome, Store,
};
use serde::Deserialize;
use serde_json::Number;
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) const IMPORT_USAGE: &str =
    "agenturm import-evals --file <results.json|-> --db <path> --source-id otlp.<id>";
pub(super) const QUERY_USAGE: &str = "agenturm evals --db <path> --source-id otlp.<id> [--trace-id <id> [--history [--before-revision <n>]] | --summary [--after-trace-id <id>]] [--limit <1..256>] [--json]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvalOptions {
    db: PathBuf,
    source_id: ExtensionId,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Import(PathBuf),
    Read { trace_id: String, json: bool },
    Browse { query: EvalQuery, json: bool },
}

pub(super) fn parse(arguments: &[String], import: bool) -> Result<EvalOptions, String> {
    let usage = if import { IMPORT_USAGE } else { QUERY_USAGE };
    let (mut db, mut source, mut value) = (None, None, None);
    let (mut json, mut history, mut summary) = (false, false, false);
    let (mut limit, mut before, mut after) = (None, None, None);
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        if !import {
            let boolean = match flag.as_str() {
                "--json" => Some(&mut json),
                "--history" => Some(&mut history),
                "--summary" => Some(&mut summary),
                _ => None,
            };
            if let Some(boolean) = boolean {
                if *boolean {
                    return Err(usage.into());
                }
                *boolean = true;
                continue;
            }
        }
        let argument = arguments
            .next()
            .filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
            .ok_or(usage)?;
        let slot = match flag.as_str() {
            "--db" if argument != "-" && argument != ":memory:" => &mut db,
            "--source-id" => &mut source,
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
    let action = if import {
        Action::Import(PathBuf::from(value.ok_or(usage)?))
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
        Action::Browse { query, json }
    } else {
        if limit.is_some() || before.is_some() || after.is_some() {
            return Err(usage.into());
        }
        Action::Read {
            trace_id: value.ok_or(usage)?.to_ascii_lowercase(),
            json,
        }
    };
    Ok(EvalOptions {
        db: PathBuf::from(db.ok_or(usage)?),
        source_id: crate::trace_import::parse_source_id(source.ok_or(usage)?)?,
        action,
    })
}

pub fn execute_evals(options: &EvalOptions) -> Result<String, String> {
    match &options.action {
        Action::Import(path) => {
            let outcomes = prepare(path)?;
            if outcomes.is_empty() {
                return Ok(report(EvalImportReport::default()));
            }
            // Evaluation records are attached to existing history only.
            Store::open_read_only(&options.db).map_err(history_error)?;
            let mut store = Store::open_collector(&options.db).map_err(history_error)?;
            let imported = store
                .import_eval_outcomes(&options.source_id, &outcomes)
                .map_err(|error| error.to_string())?;
            Ok(report(imported))
        }
        Action::Browse { query, json } => {
            let store = Store::open_read_only(&options.db).map_err(history_error)?;
            let page = store
                .query_eval_outcomes(&options.source_id, query)
                .map_err(|error| error.to_string())?;
            display_page(page, query, *json)
        }
        Action::Read { trace_id, json } => {
            let store = Store::open_read_only(&options.db).map_err(history_error)?;
            let outcome = store
                .eval_outcome(&options.source_id, trace_id)
                .map_err(|error| error.to_string())?;
            display(outcome.as_ref(), *json)
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &EvalOptions,
    store: &agenturm_storage_service::NativeStore,
) -> Result<String, String> {
    match &options.action {
        Action::Import(path) => {
            let outcomes = prepare(path)?;
            let imported = store
                .import_eval_outcomes(&options.source_id, &outcomes)
                .map_err(|error| error.to_string())?;
            Ok(report(imported))
        }
        Action::Browse { query, json } => {
            let page = store
                .query_eval_outcomes(&options.source_id, query)
                .map_err(|error| error.to_string())?;
            display_page(page, query, *json)
        }
        Action::Read { trace_id, json } => {
            let outcome = store
                .eval_outcome(&options.source_id, trace_id)
                .map_err(|error| error.to_string())?;
            display(outcome.as_ref(), *json)
        }
    }
}

fn history_error(error: agenturm_store::StoreError) -> String {
    format!("open history failed: {error}")
}

fn prepare(path: &Path) -> Result<Vec<EvalOutcome>, String> {
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
    let file: Results = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Invalid evaluation results at line {} column {}",
            error.line(),
            error.column()
        )
    })?;
    if file.results.len() > MAX_EVAL_OUTCOMES {
        return Err("Evaluation results exceed 1024 outcomes".into());
    }
    file.results
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let outcome = record.into_outcome();
            outcome
                .validate()
                .map_err(|_| format!("Invalid evaluation metadata in result {}", index + 1))?;
            Ok(outcome)
        })
        .collect()
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
    let outcome = &evaluation.outcome;
    let status = if outcome.passed { "passed" } else { "failed" };
    let mut output = format!(
        "Evaluation {status}\nTrace {} {}\nRevision {}\n",
        evaluation.trace_source_id.as_str(),
        outcome.trace_id,
        evaluation.revision,
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
    Ok(output)
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
