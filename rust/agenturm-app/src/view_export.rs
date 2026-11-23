//! Export the metadata a reader has already loaded, without rereading sources.
use agenturm_ui::{LoadedView, WorkResult};
use std::path::Path;

pub(super) fn export(document: &LoadedView, directory: &str) -> Result<WorkResult, String> {
    let text = document.plain_text()?;
    let markdown = format!(
        "# Loaded view\n\n{}",
        text.lines()
            .map(|line| format!("    {line}\n"))
            .collect::<String>()
    );
    let html = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Agenturm loaded view</title><style>\
         :root{{color-scheme:light dark}}body{{font:16px/1.6 system-ui;margin:2rem auto;\
         padding:0 1rem;max-width:70rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;\
         background:light-dark(#f2f0eb,#191822);padding:1rem;border-radius:.5rem}}\
         </style><h1>Loaded view</h1><pre>{}</pre></html>",
        escape_html(&text)
    );
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "format": "agenturm.loaded-view/1",
        "coverage": if document.truncated { "partial_loaded_page" } else { "loaded_page" },
        "evidence_receipt": false,
        "text": text,
    }))
    .map_err(|_| "Cannot encode loaded view")?;
    let destination = super::handoff_export::export_documents(
        Path::new(directory),
        &[
            ("view.md", &markdown),
            ("view.html", &html),
            ("view.json", &json),
        ],
    )
    .map_err(|error| format!("Cannot export loaded view: {error}"))?;
    Ok(WorkResult::Status(format!(
        "Saved loaded metadata view to {}",
        destination.display()
    )))
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::MetricRow;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn exports_are_private_escaped_and_never_overwrite_a_destination() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "agenturm-loaded-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let destination = root.join("view export");
        let view = LoadedView {
            title: "Traces".into(),
            scope: "loaded snapshot".into(),
            rows: vec![MetricRow {
                label: "Value".into(),
                value: "<script>alert('fixture')</script>".into(),
            }],
            truncated: true,
        };
        let path = destination.to_str().unwrap();
        assert!(export(&view, path).is_ok());
        let html = std::fs::read_to_string(destination.join("view.html")).unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        let before = std::fs::read(destination.join("view.json")).unwrap();
        assert!(export(&view, path).is_err());
        assert_eq!(
            before,
            std::fs::read(destination.join("view.json")).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&destination)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
