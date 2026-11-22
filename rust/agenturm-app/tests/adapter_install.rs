#![cfg(target_os = "linux")]

use agenturm_kernel::ObservationStore;
use agenturm_store::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Directory(PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn independently_built_adapter_installs_and_is_explicitly_selected_for_collection() {
    let (Ok(host), Ok(executable)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_STANDALONE_ADAPTER"),
    ) else {
        eprintln!("set the plugin host and independently built adapter paths");
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Directory(std::env::temp_dir().join(format!(
        "agenturm-adapter-install-cli-{}-{nonce}",
        std::process::id()
    )));
    let package = root.0.join("package");
    fs::create_dir_all(&package).unwrap();
    let binary = package.join("standalone-test-harness");
    fs::copy(executable, &binary).unwrap();
    let digest = format!("{:x}", Sha256::digest(fs::read(&binary).unwrap()));
    let manifest = package.join("manifest.json");
    let mut declaration = json!({
        "version":1,"plugin_id":"fixture.standalone-harness",
        "platform":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),
        "adapter":"standalone-test-harness","sha256":digest,"protocol":1,
        "features":["source-adapter/1","configured-root/1"],
        "observation_format":"agenturm-observations/1",
    });
    fs::write(&manifest, declaration.to_string()).unwrap();

    let app = env!("CARGO_BIN_EXE_agenturm");
    let managed = root.0.join("managed");
    let install = |destination: &Path| {
        Command::new(app)
            .args(["adapter", "install", "--sha256", &digest, "--manifest"])
            .arg(&manifest)
            .arg("--root")
            .arg(destination)
            .output()
            .unwrap()
    };
    let output = install(&managed);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalog = managed.join("catalog.json");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        catalog.to_str().unwrap()
    );
    fs::remove_file(&binary).unwrap();
    let source = root.0.join("native-source.txt");
    let config = root.0.join("sources.json");
    let db = root.0.join("history.sqlite");
    fs::write(&source, "tokens=42\n").unwrap();
    let mut selection = json!({
        "version":2,"plugin_host":host,"plugin_catalog":catalog,
        "sources":[{
            "plugin_id":"fixture.standalone-harness",
            "source_id":"harness.independent",
            "source_root":source,
        }],
    });
    fs::write(&config, selection.to_string()).unwrap();
    let collect = |database: &Path| {
        Command::new(app)
            .args(["collect", "--config"])
            .arg(&config)
            .arg("--db")
            .arg(database)
            .output()
            .unwrap()
    };
    let output = collect(&db);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("ACK sent=true"));
    let store = Store::open_read_only(&db).unwrap();
    assert_eq!(store.load_observations(10).unwrap().observations.len(), 1);
    assert!(
        store
            .source_cursor("harness.independent")
            .unwrap()
            .is_some()
    );

    let output = collect(&db);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("committed 0 observation(s)"));

    let installed = managed.join("artifacts").join(&digest);
    fs::copy(&installed, &binary).unwrap();
    declaration["features"] = json!(["source-adapter/1", "configured-root/1", "missing-feature/1"]);
    fs::write(&manifest, declaration.to_string()).unwrap();
    let mismatched = root.0.join("mismatched");
    assert!(install(&mismatched).status.success());
    selection["plugin_catalog"] = json!(mismatched.join("catalog.json"));
    fs::write(&config, selection.to_string()).unwrap();
    let mismatched_db = root.0.join("mismatched.sqlite");
    let failure = collect(&mismatched_db);
    assert!(!failure.status.success());
    assert!(
        Store::open_read_only(&mismatched_db)
            .unwrap()
            .load_observations(10)
            .unwrap()
            .observations
            .is_empty()
    );

    selection["plugin_catalog"] = json!(catalog);
    fs::write(&config, selection.to_string()).unwrap();
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(installed, b"changed executable bytes").unwrap();
    let unopened = root.0.join("unopened.sqlite");
    let failure = collect(&unopened);
    assert!(!failure.status.success());
    assert!(!unopened.exists());
}
