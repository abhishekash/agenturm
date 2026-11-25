//! Exports a saved handoff to a new local directory.

use agenturm_store::{ApprovedTaskSource, SavedHandoff, TaskHandoff, WorkspaceAnchor};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn export(db: &Path, handoff: &SavedHandoff) -> Result<PathBuf, String> {
    write_export(db, handoff).map_err(|error| format!("cannot export handoff: {error}"))
}

pub(super) fn export_task(
    destination: &Path,
    handoff: &TaskHandoff,
    history: bool,
) -> Result<PathBuf, String> {
    let (title, sections) = task_sections(handoff, history);
    export_documents(
        destination,
        &[
            ("handoff.md", &markdown_sections(&title, &sections)),
            ("handoff.html", &html_sections(&title, &sections)),
        ],
    )
    .map_err(|error| format!("cannot export task handoff: {error}"))
}

pub(super) fn export_documents(
    destination: &Path,
    documents: &[(&str, &str)],
) -> io::Result<PathBuf> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize()?;
    let name = destination.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "supply a new export directory")
    })?;
    let destination = parent.join(name);
    write_documents(&staging_dir(&parent)?, &destination, documents)?;
    Ok(destination)
}

pub(super) fn render_task(handoff: &TaskHandoff, history: bool, html: bool) -> String {
    let (title, sections) = task_sections(handoff, history);
    if html {
        html_sections(&title, &sections)
    } else {
        markdown_sections(&title, &sections)
    }
}

pub(super) fn render_saved(handoff: &SavedHandoff, html: bool) -> String {
    let title = format!("Handoff {}", handoff.id);
    let sections = [
        ("From", anchor_text(&handoff.from)),
        ("To", anchor_text(&handoff.to)),
        ("Note", handoff.note.clone()),
    ];
    if html {
        html_sections(&title, &sections)
    } else {
        markdown_sections(&title, &sections)
    }
}

fn write_export(db: &Path, handoff: &SavedHandoff) -> io::Result<PathBuf> {
    let db = db.canonicalize()?;
    if !db.is_file() {
        return Err(io::Error::other("history path must be a regular file"));
    }
    let parent = db
        .parent()
        .ok_or_else(|| io::Error::other("history path has no parent"))?;
    let root = parent.join("handoffs");
    create_export_root(&root)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        "handoff-{}-{timestamp}-{}-{sequence}",
        handoff.id,
        std::process::id()
    );
    let destination = root.join(&name);
    let staging = staging_dir(&root)?;
    let title = format!("Handoff {}", handoff.id);
    let sections = [
        ("From", anchor_text(&handoff.from)),
        ("To", anchor_text(&handoff.to)),
        ("Note", handoff.note.clone()),
    ];
    write_documents(
        &staging,
        &destination,
        &[
            ("handoff.md", &markdown_sections(&title, &sections)),
            ("handoff.html", &html_sections(&title, &sections)),
        ],
    )?;
    sync_dir(parent).map_err(|error| {
        io::Error::other(format!(
            "files were published but the export directory could not be synced: {error}"
        ))
    })?;
    Ok(destination)
}

fn staging_dir(parent: &Path) -> io::Result<Staging> {
    let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let staging = parent.join(format!(
        ".agenturm-handoff-{timestamp}-{}-{sequence}.pending",
        std::process::id()
    ));
    private_dir().create(&staging)?;
    Ok(Staging(staging))
}

fn write_documents(
    staging: &Staging,
    destination: &Path,
    documents: &[(&str, &str)],
) -> io::Result<()> {
    for (name, contents) in documents {
        write_private(&staging.0.join(name), contents)?;
    }
    sync_dir(&staging.0)?;
    publish(&staging.0, destination, documents)?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("export path has no parent"))?;
    sync_dir(parent).map_err(|error| {
        io::Error::other(format!(
            "files were published but the export directory could not be synced: {error}"
        ))
    })
}

fn create_export_root(root: &Path) -> io::Result<()> {
    match private_dir().create(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = root.symlink_metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("handoffs path must be a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "handoffs directory permissions must be private",
            ));
        }
    }
    Ok(())
}

fn private_dir() -> DirBuilder {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(not(unix))]
    let _ = &mut builder;
    builder
}

fn write_private(path: &Path, data: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data.as_bytes())?;
    file.sync_all()
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(target_os = "linux")]
fn publish(staging: &Path, destination: &Path, _: &[(&str, &str)]) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, staging, CWD, destination, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn publish(staging: &Path, destination: &Path, documents: &[(&str, &str)]) -> io::Result<()> {
    private_dir().create(destination)?;
    for (name, _) in documents {
        if let Err(error) = fs::hard_link(staging.join(name), destination.join(name)) {
            let _ = fs::remove_dir_all(destination);
            return Err(error);
        }
    }
    sync_dir(destination)
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn markdown_sections(title: &str, sections: &[(&str, String)]) -> String {
    let mut output = format!("# {title}\n");
    for (label, text) in sections {
        let mut run = 0;
        let mut longest = 2;
        for byte in text.bytes() {
            run = if byte == b'`' { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        let fence = "`".repeat(longest + 1);
        let _ = write!(output, "\n## {label}\n\n{fence}\n{text}\n{fence}\n");
    }
    output
}

fn anchor_text(anchor: &WorkspaceAnchor) -> String {
    let mut output = format!(
        "Source  {}\nSession {}",
        anchor.harness_id.as_str(),
        anchor.session_id
    );
    if let Some(worker) = &anchor.worker_id {
        let _ = write!(output, "\nWorker  {worker}");
    }
    if let Some(turn) = &anchor.turn_id {
        let _ = write!(output, "\nTurn    {turn}");
    }
    output
}

fn task_sections(handoff: &TaskHandoff, history: bool) -> (String, Vec<(&'static str, String)>) {
    let task = &handoff.task;
    let title = format!("Task {} handoff", task.id);
    let status = if task.completed { "Completed" } else { "Open" };
    let archived = if task.archived { " · Archived" } else { "" };
    let mut sections = vec![(
        "Task",
        format!(
            "{}\nRevision {}\n{status}{archived}",
            task.title, task.revision
        ),
    )];
    let brief = &handoff.brief;
    for (label, text) in [
        ("Notes", &task.note),
        ("Intent", &brief.intent),
        ("Acceptance criteria", &brief.acceptance_criteria),
        ("Constraints", &brief.constraints),
        ("Open questions", &brief.open_questions),
        ("Next action", &brief.next_action),
    ] {
        if !text.is_empty() {
            sections.push((label, text.clone()));
        }
    }
    if !handoff.links.is_empty() {
        let mut links = handoff
            .links
            .iter()
            .map(anchor_text)
            .collect::<Vec<_>>()
            .join("\n\n");
        if handoff.links_truncated {
            links.push_str("\n\nMore linked runs are available in the task ledger.");
        }
        sections.push(("Linked runs", links));
    }
    if let Some(approval) = handoff.continuity.last() {
        let selected = match &approval.source {
            ApprovedTaskSource::Native(source) => format!(
                "{}\nEvent {} · revision {}",
                anchor_text(&source.anchor),
                source.event_id,
                source.revision
            ),
            ApprovedTaskSource::Python(source) => format!(
                "Python receiver {}\n{} / {}\nRecorded event {}",
                source.scope.receiver,
                source.scope.producer,
                source.scope.session_id,
                source.occurrence.source_id
            ),
        };
        let mut text = format!("Approved continuation {}\n{selected}", approval.id);
        if !approval.continuation.is_empty() {
            let _ = write!(text, "\n\n{}", approval.continuation);
        }
        sections.push(("Latest approved continuation", text));
    }
    let superseded: BTreeSet<_> = handoff
        .decisions
        .iter()
        .filter_map(|item| item.decision.supersedes_id)
        .collect();
    let mut decisions = Vec::new();
    for item in &handoff.decisions {
        let replaced = superseded.contains(&item.id);
        if !history && replaced {
            continue;
        }
        let decision = &item.decision;
        let status = if replaced { "superseded" } else { "current" };
        let mut text = format!(
            "{} · {} · {status}\n{}",
            item.id,
            decision.kind.as_str(),
            decision.text
        );
        if !decision.rationale.is_empty() {
            let _ = write!(text, "\nRationale\n{}", decision.rationale);
        }
        if !decision.scope.is_empty() {
            let _ = write!(text, "\nScope\n{}", decision.scope);
        }
        if let Some(previous) = decision.supersedes_id {
            let _ = write!(text, "\nSupersedes {previous}");
        }
        if let Some(source) = &item.source {
            let _ = write!(
                text,
                "\nSource reference\n{}\nEvent {} · revision {}",
                anchor_text(&source.anchor),
                source.event_id,
                source.revision
            );
        }
        decisions.push(text);
    }
    if !decisions.is_empty() {
        sections.push(("Decisions", decisions.join("\n\n")));
    }
    if let Some(git) = &handoff.git_snapshot {
        let mut text = format!(
            "{}\nStaged {} · Unstaged {} · Untracked {}\nCaptured at {} Unix ns",
            git.repository_root,
            git.staged_changes,
            git.unstaged_changes,
            git.untracked_files,
            git.captured_at_unix_ns
        );
        if let Some(branch) = &git.branch {
            let _ = write!(text, "\nBranch {branch}");
        }
        if let Some(head) = &git.head_commit {
            let _ = write!(text, "\nHEAD {head}");
        }
        sections.push(("Local Git summary", text));
    }
    if let Some(diff) = &handoff.git_diff {
        sections.push(("Staged Git patch", diff.staged.clone()));
        sections.push(("Unstaged Git patch", diff.unstaged.clone()));
    }
    (title, sections)
}

pub(super) fn html_sections(title: &str, sections: &[(&str, String)]) -> String {
    let mut output = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{title}</title>\n\
         <style>html{{color-scheme:light dark}}body{{font:1rem/1.5 system-ui,sans-serif;max-width:54rem;margin:3rem auto;padding:0 1.25rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;border:1px solid GrayText;border-radius:.25rem;padding:1rem}}</style>\n\
         </head><body><main><h1>{title}</h1>\n",
    );
    for (label, text) in sections {
        let _ = write!(output, "<section><h2>{label}</h2><pre>");
        escape_html(&mut output, text);
        output.push_str("</pre></section>\n");
    }
    output.push_str("</main></body></html>\n");
    output
}

fn escape_html(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            character => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ExtensionId;

    #[test]
    fn export_keeps_literal_text_and_never_overwrites_an_earlier_pair() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        File::create(&db).unwrap();
        let from = WorkspaceAnchor {
            harness_id: ExtensionId::parse("harness.codex").unwrap(),
            session_id: "from<session>".into(),
            worker_id: Some("worker".into()),
            turn_id: Some("turn".into()),
        };
        let to = WorkspaceAnchor {
            harness_id: ExtensionId::parse("harness.kiro").unwrap(),
            session_id: "target".into(),
            worker_id: None,
            turn_id: None,
        };
        let inner_fence = "\x60".repeat(3);
        let mut handoff = SavedHandoff {
            id: 9,
            from,
            to,
            note: format!(
                "{inner_fence}\n</pre><script>alert(\"x\")</script> & 'literal'\n{inner_fence}"
            ),
        };

        let first = export(&db, &handoff).unwrap();
        assert_eq!(first.parent(), Some(scratch.0.join("handoffs").as_path()));
        let markdown = fs::read_to_string(first.join("handoff.md")).unwrap();
        let html = fs::read_to_string(first.join("handoff.html")).unwrap();
        let outer_fence = "\x60".repeat(4);
        assert!(markdown.contains(&format!(
            "\n{outer_fence}\n{}\n{outer_fence}\n",
            handoff.note
        )));
        assert!(markdown.contains("Worker  worker\nTurn    turn"));
        assert!(html.contains("from&lt;session&gt;"));
        assert!(html.contains(
            "&lt;/pre&gt;&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;literal&#39;"
        ));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("href="));
        handoff.note = "updated local note".into();
        let second = export(&db, &handoff).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(first.join("handoff.md")).unwrap(),
            markdown
        );
        assert!(
            fs::read_to_string(second.join("handoff.md"))
                .unwrap()
                .contains(&handoff.note)
        );
        assert_eq!(fs::read_dir(first.parent().unwrap()).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for directory in [first.parent().unwrap(), first.as_path(), second.as_path()] {
                assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
            }
            for file in [first.join("handoff.md"), first.join("handoff.html")] {
                assert_eq!(fs::metadata(file).unwrap().mode() & 0o777, 0o600);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_symlink_cannot_receive_private_exports() {
        use std::os::unix::fs::symlink;

        let scratch = Scratch::new();
        let external = scratch.0.join("external");
        fs::create_dir(&external).unwrap();
        symlink(&external, scratch.0.join("handoffs")).unwrap();
        assert!(create_export_root(&scratch.0.join("handoffs")).is_err());
        assert_eq!(fs::read_dir(external).unwrap().count(), 0);
    }

    #[test]
    fn publication_preserves_an_existing_destination() {
        let scratch = Scratch::new();
        let staging = scratch.0.join("staging");
        let destination = scratch.0.join("destination");
        private_dir().create(&staging).unwrap();
        private_dir().create(&destination).unwrap();
        assert_eq!(
            publish(&staging, &destination, &[]).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(staging.is_dir());
        assert!(destination.is_dir());
    }

    #[test]
    fn document_export_refuses_an_existing_pair_and_keeps_the_first_copy() {
        let scratch = Scratch::new();
        let destination = scratch.0.join("preview");
        export_documents(&destination, &[("handoff.md", "first")]).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("handoff.md")).unwrap(),
            "first"
        );
        let error = export_documents(&destination, &[("handoff.md", "second")]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(destination.join("handoff.md")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_dir(&scratch.0)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".pending"))
                .count(),
            0
        );
    }

    #[test]
    fn decision_history_labels_current_and_superseded_versions_exactly() {
        use agenturm_store::{SavedTaskDecision, TaskBrief, TaskDecisionInput, TaskDecisionKind};

        let handoff = TaskHandoff {
            task: agenturm_store::SavedTask {
                id: 4,
                anchor: None,
                title: "Retain the API".into(),
                note: "Operator note".into(),
                completed: false,
                archived: false,
                revision: 7,
                workspace_directory: None,
            },
            brief: TaskBrief {
                intent: "Avoid a breaking change".into(),
                ..TaskBrief::default()
            },
            decisions: vec![
                SavedTaskDecision {
                    id: 1,
                    created_at_unix_ns: 1,
                    decision: TaskDecisionInput {
                        kind: TaskDecisionKind::Accepted,
                        text: "Keep the old endpoint".into(),
                        rationale: String::new(),
                        scope: String::new(),
                        supersedes_id: None,
                    },
                    source: None,
                },
                SavedTaskDecision {
                    id: 2,
                    created_at_unix_ns: 2,
                    decision: TaskDecisionInput {
                        kind: TaskDecisionKind::Rejected,
                        text: "Remove the endpoint".into(),
                        rationale: "Deprecate first".into(),
                        scope: "Public callers".into(),
                        supersedes_id: Some(1),
                    },
                    source: None,
                },
            ],
            links: Vec::new(),
            links_truncated: false,
            git_snapshot: None,
            git_diff: None,
            continuity: Vec::new(),
        };
        let current = render_task(&handoff, false, false);
        assert!(current.contains("Remove the endpoint"));
        assert!(current.contains("current"));
        assert!(!current.contains("Keep the old endpoint"));
        let history = render_task(&handoff, true, true);
        assert!(history.contains("Keep the old endpoint"));
        assert!(history.contains("superseded"));
        assert!(history.contains("Remove the endpoint"));
        assert!(history.contains("Supersedes 1"));
        assert!(history.contains("Revision 7"));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "agenturm-handoff-export-{}-{sequence}",
                std::process::id()
            ));
            private_dir().create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
