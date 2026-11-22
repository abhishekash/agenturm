//! Exports a saved handoff to a new local directory.

use agenturm_store::{SavedHandoff, WorkspaceAnchor};
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
    let staging_path = root.join(format!(".{name}.pending"));
    private_dir().create(&staging_path)?;
    let staging = Staging(staging_path);
    write_private(&staging.0.join("handoff.md"), &markdown(handoff))?;
    write_private(&staging.0.join("handoff.html"), &html(handoff))?;
    sync_dir(&staging.0)?;
    publish(&staging.0, &destination)?;
    sync_dir(&root)
        .and_then(|()| sync_dir(parent))
        .map_err(|error| {
            io::Error::other(format!(
                "files were published but the export directory could not be synced: {error}"
            ))
        })?;
    Ok(destination)
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
fn publish(staging: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, staging, CWD, destination, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn publish(staging: &Path, destination: &Path) -> io::Result<()> {
    private_dir().create(destination)?;
    for name in ["handoff.md", "handoff.html"] {
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

fn markdown(handoff: &SavedHandoff) -> String {
    let mut output = format!("# Handoff {}\n", handoff.id);
    for (label, text) in [
        ("From", anchor_text(&handoff.from)),
        ("To", anchor_text(&handoff.to)),
        ("Note", handoff.note.clone()),
    ] {
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

fn html(handoff: &SavedHandoff) -> String {
    let mut output = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>Handoff {0}</title>\n\
         <style>html{{color-scheme:light dark}}body{{font:1rem/1.5 system-ui,sans-serif;max-width:54rem;margin:3rem auto;padding:0 1.25rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;border:1px solid GrayText;border-radius:.25rem;padding:1rem}}</style>\n\
         </head><body><main><h1>Handoff {0}</h1>\n",
        handoff.id
    );
    for (label, text) in [
        ("From", anchor_text(&handoff.from)),
        ("To", anchor_text(&handoff.to)),
        ("Note", handoff.note.clone()),
    ] {
        let _ = write!(output, "<section><h2>{label}</h2><pre>");
        escape_html(&mut output, &text);
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
            publish(&staging, &destination).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(staging.is_dir());
        assert!(destination.is_dir());
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
