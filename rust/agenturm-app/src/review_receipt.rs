//! Formats a bounded Review page from the metadata-only history projection.

use crate::handoff_export::{export_documents, html_sections, markdown_sections};
use serde_json::{Value, json};
use std::path::PathBuf;

const TITLE: &str = "Review receipt";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReviewFormat {
    Json,
    Text,
    Markdown,
    Html,
}

impl ReviewFormat {
    pub(super) fn parse(format: &str) -> Result<Self, String> {
        match format {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            _ => Err("Choose a review format from text, json, markdown, or html".into()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReviewOutput {
    pub(super) format: Option<ReviewFormat>,
    pub(super) directory: Option<PathBuf>,
}

pub(super) struct ReviewScope<'a> {
    pub harness_id: &'a str,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub worker_id: Option<&'a str>,
}

pub(super) fn render(
    mut page: Value,
    scope: ReviewScope<'_>,
    snapshot_id: u64,
    output: &ReviewOutput,
) -> Result<String, String> {
    let encode = |value: &Value| {
        serde_json::to_string_pretty(value).map_err(|_| "Cannot encode review receipt".to_owned())
    };
    let legacy = if output.format.is_none() {
        let existing = encode(&page)?;
        if output.directory.is_none() {
            return Ok(existing);
        }
        Some(existing)
    } else {
        None
    };
    page["format"] = json!("agenturm-review-receipt/1");
    page["snapshot_id"] = json!(snapshot_id.to_string());
    page["scope"] = json!({
        "harness_id": scope.harness_id,
        "session_id": scope.session_id,
        "turn_id": scope.turn_id,
        "worker_id": scope.worker_id,
    });
    if output.format == Some(ReviewFormat::Json) && output.directory.is_none() {
        return encode(&page);
    }
    let worker = page["scope"]["worker_id"]
        .as_str()
        .map_or_else(|| "Workerless".into(), |id| json!(id).to_string());
    let scope_summary = format!(
        "Source   {}\nSession  {}\nTurn     {}\nWorker   {}\nSnapshot {}",
        page["scope"]["harness_id"],
        page["scope"]["session_id"],
        page["scope"]["turn_id"],
        worker,
        snapshot_id
    );
    let mut sections = vec![("Scope", scope_summary)];
    if !page["next_before"].is_null() {
        sections.push(("Older evidence cursor", encode(&page["next_before"])?));
    }
    sections.push(("Recorded observations", encode(&page["observations"])?));

    let receipt = |format| match format {
        ReviewFormat::Json => encode(&page),
        ReviewFormat::Text => {
            let mut text = format!("{TITLE}\n");
            for (label, content) in &sections {
                text.push_str(&format!("\n{label}\n{content}\n"));
            }
            Ok(text)
        }
        ReviewFormat::Markdown => Ok(markdown_sections(TITLE, &sections)),
        ReviewFormat::Html => Ok(html_sections(TITLE, &sections)),
    };
    if let Some(path) = &output.directory {
        let markdown = receipt(ReviewFormat::Markdown)?;
        let html = receipt(ReviewFormat::Html)?;
        let json = receipt(ReviewFormat::Json)? + "\n";
        export_documents(
            path,
            &[
                ("review.md", &markdown),
                ("review.html", &html),
                ("review.json", &json),
            ],
        )
        .map_err(|error| format!("Cannot export review receipt: {error}"))?;
    }
    if let Some(legacy) = legacy {
        Ok(legacy)
    } else {
        receipt(output.format.unwrap_or(ReviewFormat::Json))
    }
}
