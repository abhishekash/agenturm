//! Local display preference for authenticated native internal review metadata.

use crate::theme;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 32;

pub(super) fn path() -> Option<PathBuf> {
    theme::preferences_enabled()
        .then(theme::preference_path)
        .flatten()
        .map(|path| path.with_file_name("include-reviews"))
}

pub(super) fn read(path: &Path) -> bool {
    fn read_value(path: &Path) -> io::Result<bool> {
        let metadata = path.symlink_metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return Ok(false);
        }
        let mut contents = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut contents)?;
        Ok(contents == b"true\n")
    }
    read_value(path).unwrap_or(false)
}

pub(super) fn update(path: Option<&Path>, include: bool) -> Result<(), String> {
    if let Some(path) = path {
        save(path, include).map_err(|_| "Could not save the internal review display preference")?;
    }
    Ok(())
}

fn save(path: &Path, include: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing configuration directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".include-reviews-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        writeln!(file, "{include}")?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_review_visibility_defaults_off_and_survives_reopen() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-test-review-preference-{}",
            std::process::id()
        ));
        let preference = root.join("include-reviews");
        assert!(!read(&preference));
        update(Some(&preference), true).unwrap();
        assert!(read(&preference));
        update(Some(&preference), false).unwrap();
        assert!(!read(&preference));
        std::fs::remove_file(preference).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
