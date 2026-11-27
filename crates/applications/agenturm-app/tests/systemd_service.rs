#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "agenturm-systemd-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn cli_installs_reports_and_uninstalls_the_user_unit() {
    let root = temporary_directory();
    let fake_bin = root.join("fake bin");
    let config_home = root.join("xdg config");
    fs::create_dir_all(&fake_bin).expect("create fake systemctl directory");
    let systemctl_log = root.join("systemctl.log");
    let systemctl = fake_bin.join("systemctl");
    fs::write(
        &systemctl,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$AGENTURM_SYSTEMCTL_LOG\"\nif [ \"$2\" = \"status\" ]; then printf 'inactive (dead)\\n'; exit 3; fi\n",
    )
    .expect("write systemctl shim");
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700))
        .expect("make systemctl shim executable");
    let config = root.join("sources.json");
    fs::write(
        &config,
        format!(
            r#"{{"version":1,"plugin_host":"/bin/true","sources":[{{"plugin_id":"example.rust-pi-harness","adapter":"/bin/true","source_root":"{}","source_id":"harness.pi"}}]}}"#,
            root.join("sources").display()
        ),
    )
    .expect("write collection config");
    let database = root.join("state data/agenturm.sqlite");
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path_entries = std::env::split_paths(&original_path).collect::<Vec<_>>();
    path_entries.insert(0, fake_bin.clone());
    let path = std::env::join_paths(path_entries).expect("compose fake command PATH");
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args(arguments)
            .env("PATH", &path)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("AGENTURM_SYSTEMCTL_LOG", &systemctl_log)
            .output()
            .expect("run Agenturm service command")
    };

    let install = run(&[
        "service",
        "install",
        "--config",
        config.to_str().unwrap(),
        "--db",
        database.to_str().unwrap(),
    ]);
    assert!(
        install.status.success(),
        "service install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    let unit = config_home
        .join("systemd/user")
        .join("agenturm-collector.service");
    let unit_contents = fs::read_to_string(&unit).expect("read installed unit");
    assert!(unit_contents.contains("--watch --interval-ms 2000"));
    assert!(unit_contents.contains(database.to_str().unwrap()));
    assert_eq!(
        fs::metadata(&unit).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let status = run(&["service", "status"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("inactive (dead)"));

    let uninstall = run(&["service", "uninstall"]);
    assert!(
        uninstall.status.success(),
        "service uninstall failed: {}",
        String::from_utf8_lossy(&uninstall.stderr)
    );
    assert!(!unit.exists());
    assert_eq!(
        fs::read_to_string(&systemctl_log).unwrap(),
        concat!(
            "--user daemon-reload\n",
            "--user enable --now agenturm-collector.service\n",
            "--user status --no-pager agenturm-collector.service\n",
            "--user disable --now agenturm-collector.service\n",
            "--user daemon-reload\n",
            "--user reset-failed agenturm-collector.service\n"
        )
    );

    let overlapping_config = root.join("overlapping-sources.json");
    let overlapping_source_root = root.join("new-source-root");
    let overlapping_database = overlapping_source_root.join("agenturm.sqlite");
    fs::write(
        &overlapping_config,
        format!(
            r#"{{"version":1,"plugin_host":"/bin/true","sources":[{{"plugin_id":"example.rust-pi-harness","adapter":"/bin/true","source_root":"{}","source_id":"harness.pi"}}]}}"#,
            overlapping_source_root.display()
        ),
    )
    .expect("write overlapping collection config");
    let overlap = run(&[
        "service",
        "install",
        "--config",
        overlapping_config.to_str().unwrap(),
        "--db",
        overlapping_database.to_str().unwrap(),
    ]);
    assert!(!overlap.status.success());
    assert!(String::from_utf8_lossy(&overlap.stderr).contains("must not overlap"));
    assert!(!overlapping_source_root.exists());
    fs::remove_dir_all(root).expect("remove service fixture");
}
