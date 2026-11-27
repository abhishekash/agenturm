//! User-scoped OS service lifecycle for the foreground Agenturm collector.
//!
//! The service manager is kept outside the collection kernel so other service
//! backends can be added without changing harness or storage code.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const UNIT_NAME: &str = "agenturm-collector.service";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceSpec {
    pub executable: PathBuf,
    pub collection_config: PathBuf,
    pub database: PathBuf,
    pub interval_ms: u64,
}

pub trait ServiceManager {
    fn run(&mut self, arguments: &[&str]) -> Result<String, String>;
}

#[derive(Default)]
pub struct Systemctl;

impl ServiceManager for Systemctl {
    fn run(&mut self, arguments: &[&str]) -> Result<String, String> {
        let output = Command::new("systemctl")
            .args(arguments)
            .output()
            .map_err(|error| format!("could not run systemctl: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let status_query = arguments.contains(&"status");
        if output.status.success() || (status_query && matches!(output.status.code(), Some(3 | 4)))
        {
            if status_query && !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                return Ok(if stdout.is_empty() { stderr } else { stdout });
            }
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let details = if stderr.is_empty() { stdout } else { stderr };
            Err(format!(
                "systemctl {} failed{}",
                arguments.join(" "),
                if details.is_empty() {
                    String::new()
                } else {
                    format!(": {details}")
                }
            ))
        }
    }
}

pub fn render_systemd_unit(spec: &ServiceSpec) -> Result<String, String> {
    validate_spec(spec)?;
    let executable = systemd_word(&spec.executable)?;
    let config = systemd_word(&spec.collection_config)?;
    let database = systemd_word(&spec.database)?;
    let database_directory = spec
        .database
        .parent()
        .ok_or_else(|| "database path must have a parent directory".to_owned())?;
    let writable_directory = systemd_word(database_directory)?;
    Ok(format!(
        "[Unit]\n\
Description=Agenturm Rust harness collector\n\
After=default.target\n\
StartLimitIntervalSec=60\n\
StartLimitBurst=5\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={executable} collect --config {config} --db {database} --watch --interval-ms {}\n\
Restart=on-failure\n\
RestartSec=5s\n\
TimeoutStopSec=20s\n\
KillMode=control-group\n\
UMask=0077\n\
NoNewPrivileges=true\n\
ProtectSystem=strict\n\
ProtectHome=read-only\n\
ReadWritePaths={writable_directory}\n\
RestrictSUIDSGID=true\n\
LockPersonality=true\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        spec.interval_ms
    ))
}

pub fn install_user_service(
    spec: &ServiceSpec,
    unit_directory: &Path,
    manager: &mut impl ServiceManager,
) -> Result<PathBuf, String> {
    let contents = render_systemd_unit(spec)?;
    if !unit_directory.is_absolute() {
        return Err("systemd user unit directory must be absolute".into());
    }
    fs::create_dir_all(unit_directory)
        .map_err(|error| format!("create systemd user unit directory failed: {error}"))?;
    let unit_path = unit_directory.join(UNIT_NAME);
    let existed = unit_path.exists();
    atomic_write_private(&unit_path, contents.as_bytes())?;
    manager.run(&["--user", "daemon-reload"]).map_err(|error| {
        format!(
            "service unit was written to {} but {error}",
            unit_path.display()
        )
    })?;
    if existed {
        manager
            .run(&["--user", "enable", UNIT_NAME])
            .map_err(|error| {
                format!(
                    "service unit is installed at {} but {error}",
                    unit_path.display()
                )
            })?;
        manager
            .run(&["--user", "restart", UNIT_NAME])
            .map_err(|error| {
                format!(
                    "service unit is installed at {} but {error}",
                    unit_path.display()
                )
            })?;
    } else {
        manager
            .run(&["--user", "enable", "--now", UNIT_NAME])
            .map_err(|error| {
                format!(
                    "service unit is installed at {} but {error}",
                    unit_path.display()
                )
            })?;
    }
    Ok(unit_path)
}

pub fn uninstall_user_service(
    unit_directory: &Path,
    manager: &mut impl ServiceManager,
) -> Result<(), String> {
    if !unit_directory.is_absolute() {
        return Err("systemd user unit directory must be absolute".into());
    }
    let unit_path = unit_directory.join(UNIT_NAME);
    if unit_path.is_file() {
        manager.run(&["--user", "disable", "--now", UNIT_NAME])?;
        fs::remove_file(&unit_path)
            .map_err(|error| format!("remove systemd user unit failed: {error}"))?;
    } else {
        // A prior uninstall may have removed the unit before daemon-reload
        // failed. Retry cleanup so stale manager state remains recoverable.
        let _ = manager.run(&["--user", "disable", "--now", UNIT_NAME]);
    }
    manager.run(&["--user", "daemon-reload"])?;
    let _ = manager.run(&["--user", "reset-failed", UNIT_NAME]);
    Ok(())
}

pub fn status_user_service(manager: &mut impl ServiceManager) -> Result<String, String> {
    let output = manager.run(&["--user", "status", "--no-pager", UNIT_NAME])?;
    Ok(format!("{output}\n"))
}

fn validate_spec(spec: &ServiceSpec) -> Result<(), String> {
    if !(100..=60_000).contains(&spec.interval_ms) {
        return Err("service interval must be between 100 and 60000 milliseconds".into());
    }
    for (label, path) in [
        ("Agenturm executable", &spec.executable),
        ("collection config", &spec.collection_config),
        ("database", &spec.database),
    ] {
        if !path.is_absolute() {
            return Err(format!("{label} path must be absolute"));
        }
        let value = path
            .to_str()
            .ok_or_else(|| format!("{label} path must be valid UTF-8"))?;
        if value.chars().any(char::is_control) {
            return Err(format!("{label} path cannot contain control characters"));
        }
    }
    if !spec.database.parent().is_some_and(Path::is_absolute) {
        return Err("database path must have an absolute parent directory".into());
    }
    Ok(())
}

fn systemd_word(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "systemd paths must be valid UTF-8".to_owned())?;
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '%' => escaped.push_str("%%"),
            '$' => escaped.push_str("$$"),
            value if value.is_control() => {
                return Err("systemd paths cannot contain control characters".into());
            }
            value => escaped.push(value),
        }
    }
    escaped.push('"');
    Ok(escaped)
}

fn atomic_write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "systemd unit path has no parent directory".to_owned())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?
        .as_nanos();
    let temporary = parent.join(format!(".{UNIT_NAME}.{}.{}.tmp", std::process::id(), nonce));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("create temporary service unit failed: {error}"))?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write service unit failed: {error}"));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("install service unit failed: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct FakeManager {
        calls: RefCell<Vec<Vec<String>>>,
        replies: VecDeque<Result<String, String>>,
    }

    impl FakeManager {
        fn new(replies: impl IntoIterator<Item = Result<String, String>>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                replies: replies.into_iter().collect(),
            }
        }
    }

    impl ServiceManager for FakeManager {
        fn run(&mut self, arguments: &[&str]) -> Result<String, String> {
            self.calls
                .borrow_mut()
                .push(arguments.iter().map(|arg| (*arg).to_owned()).collect());
            self.replies.pop_front().unwrap_or(Ok(String::new()))
        }
    }

    fn spec(root: &Path) -> ServiceSpec {
        ServiceSpec {
            executable: root.join("bin/agenturm"),
            collection_config: root.join("config/agenturm-sources.json"),
            database: root.join("data/agenturm state.sqlite"),
            interval_ms: 2_000,
        }
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "agenturm-service-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn systemd_unit_uses_quoted_absolute_args_and_hardening() {
        let directory = root().join("with space%$name");
        let unit = render_systemd_unit(&spec(&directory)).unwrap();
        assert!(unit.contains("ExecStart=\""));
        assert!(unit.contains("agenturm\" collect --config \""));
        assert!(unit.contains("agenturm state.sqlite\" --watch --interval-ms 2000"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("KillMode=control-group"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("ProtectHome=read-only"));
        assert!(unit.contains("NoNewPrivileges=true"));
        assert!(unit.contains("%%$$name"));
    }

    #[test]
    fn install_writes_private_unit_and_enables_user_service() {
        let directory = root();
        let mut manager = FakeManager::new([]);
        let unit_path = install_user_service(
            &spec(&directory),
            &directory.join("systemd/user"),
            &mut manager,
        )
        .unwrap();
        assert!(unit_path.is_file());
        assert_eq!(
            manager.calls.borrow().as_slice(),
            [
                vec!["--user", "daemon-reload"],
                vec!["--user", "enable", "--now", UNIT_NAME]
            ]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&unit_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reinstall_reloads_and_restarts_existing_service() {
        let directory = root();
        let unit_directory = directory.join("systemd/user");
        fs::create_dir_all(&unit_directory).unwrap();
        fs::write(unit_directory.join(UNIT_NAME), "old unit").unwrap();
        let mut manager = FakeManager::new([]);
        install_user_service(&spec(&directory), &unit_directory, &mut manager).unwrap();
        assert_eq!(
            manager.calls.borrow().as_slice(),
            [
                vec!["--user", "daemon-reload"],
                vec!["--user", "enable", UNIT_NAME],
                vec!["--user", "restart", UNIT_NAME]
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn uninstall_disables_service_before_removing_unit() {
        let directory = root();
        fs::create_dir_all(&directory).unwrap();
        let unit_path = directory.join(UNIT_NAME);
        fs::write(&unit_path, "unit").unwrap();
        let mut manager = FakeManager::new([
            Ok(String::new()),
            Ok(String::new()),
            Err("not failed".into()),
        ]);
        uninstall_user_service(&directory, &mut manager).unwrap();
        assert!(!unit_path.exists());
        assert_eq!(
            manager.calls.borrow().as_slice(),
            [
                vec!["--user", "disable", "--now", UNIT_NAME],
                vec!["--user", "daemon-reload"],
                vec!["--user", "reset-failed", UNIT_NAME]
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn uninstall_without_unit_retries_manager_cleanup_idempotently() {
        let directory = root();
        fs::create_dir_all(&directory).unwrap();
        let mut manager = FakeManager::new([
            Err("unit was not loaded".into()),
            Ok(String::new()),
            Err("already reset".into()),
        ]);
        uninstall_user_service(&directory, &mut manager).unwrap();
        assert_eq!(
            manager.calls.borrow().as_slice(),
            [
                vec!["--user", "disable", "--now", UNIT_NAME],
                vec!["--user", "daemon-reload"],
                vec!["--user", "reset-failed", UNIT_NAME]
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn uninstall_can_retry_after_unit_removal_before_reload_failure() {
        let directory = root();
        fs::create_dir_all(&directory).unwrap();
        let unit_path = directory.join(UNIT_NAME);
        fs::write(&unit_path, "unit").unwrap();
        let mut first_manager = FakeManager::new([Ok(String::new()), Err("reload failed".into())]);
        assert!(uninstall_user_service(&directory, &mut first_manager).is_err());
        assert!(!unit_path.exists());

        let mut retry_manager = FakeManager::new([
            Err("unit file already gone".into()),
            Ok(String::new()),
            Err("nothing to reset".into()),
        ]);
        uninstall_user_service(&directory, &mut retry_manager).unwrap();
        assert_eq!(
            retry_manager.calls.borrow().as_slice(),
            [
                vec!["--user", "disable", "--now", UNIT_NAME],
                vec!["--user", "daemon-reload"],
                vec!["--user", "reset-failed", UNIT_NAME]
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_relative_paths_invalid_intervals_and_control_characters() {
        let directory = root();
        let mut invalid = spec(&directory);
        invalid.interval_ms = 99;
        assert!(render_systemd_unit(&invalid).is_err());
        invalid = spec(&directory);
        invalid.collection_config = PathBuf::from("relative.json");
        assert!(render_systemd_unit(&invalid).is_err());
        invalid = spec(&directory);
        invalid.database = directory.join("bad\nname.sqlite");
        assert!(render_systemd_unit(&invalid).is_err());
    }

    #[test]
    fn generated_unit_passes_systemd_analyze_when_available() {
        if Command::new("systemd-analyze")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping systemd-analyze verification: systemd-analyze is unavailable");
            return;
        }
        let directory = root();
        let unusual_path = directory.join("space %$ with \" and \\ name");
        fs::create_dir_all(&unusual_path).unwrap();
        let spec = ServiceSpec {
            executable: std::env::current_exe().unwrap(),
            collection_config: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            database: unusual_path.join("agenturm state.sqlite"),
            interval_ms: 2_000,
        };
        let unit_path = directory.join(UNIT_NAME);
        fs::write(&unit_path, render_systemd_unit(&spec).unwrap()).unwrap();
        let output = Command::new("systemd-analyze")
            .args(["--user", "verify"])
            .arg(&unit_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "systemd-analyze verify failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
