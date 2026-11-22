//! Bounded import and exact display of agent-evals results.json metadata.

use agenturm_kernel::ExtensionId;
use agenturm_store::{
    EvalImportReport, EvalOutcome, EvalScore, MAX_EVAL_OUTCOMES, RetainedEvalOutcome, Store,
};
use serde::Deserialize;
use serde_json::Number;
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) const IMPORT_USAGE: &str =
    "agenturm import-evals --file <results.json|-> --db <path> --source-id otlp.<id>";
pub(super) const QUERY_USAGE: &str =
    "agenturm evals --db <path> --source-id otlp.<id> --trace-id <id> [--json]";

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
}

pub(super) fn parse(arguments: &[String], import: bool) -> Result<EvalOptions, String> {
    let usage = if import { IMPORT_USAGE } else { QUERY_USAGE };
    let (mut db, mut source, mut value) = (None, None, None);
    let mut json = false;
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        if flag == "--json" && !import && !json {
            json = true;
            continue;
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
            _ => return Err(usage.into()),
        };
        if slot.replace(argument).is_some() {
            return Err(usage.into());
        }
    }
    let value = value.ok_or(usage)?;
    Ok(EvalOptions {
        db: PathBuf::from(db.ok_or(usage)?),
        source_id: crate::trace_import::parse_source_id(source.ok_or(usage)?)?,
        action: if import {
            Action::Import(PathBuf::from(value))
        } else {
            Action::Read {
                trace_id: value.to_ascii_lowercase(),
                json,
            }
        },
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
