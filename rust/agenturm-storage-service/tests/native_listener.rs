#![cfg(target_os = "linux")]

use agenturm_storage_service_contract::native::SOCKET_NAME;
use agenturm_store::Store;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_socket(child: &mut Child, socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        if child.try_wait().unwrap().is_some() {
            let mut error = String::new();
            child
                .stderr
                .as_mut()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            panic!("native listener exited before it created the socket: {error}");
        }
        assert!(Instant::now() < deadline, "native listener did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_mode_creates_private_fresh_store_and_denies_its_own_uid() {
    // SAFETY: credential getters take no arguments.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut fixture = Fixture {
        root: std::env::temp_dir().join(format!(
            "agenturm-native-listener-{}-{nonce}",
            std::process::id()
        )),
        child: None,
    };
    let data = fixture.root.join("data");
    let transport = fixture.root.join("transport");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir(&transport).unwrap();
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&transport, fs::Permissions::from_mode(0o2710)).unwrap();
    let socket = transport.join(SOCKET_NAME);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm-storage-service"));
    command
        .arg("--data-root")
        .arg(&data)
        .arg("--transport-dir")
        .arg(&transport)
        .args([
            "--receiver-id",
            "receiver:native",
            "--client-uid",
            &(uid + 1).to_string(),
            "--client-gid",
            &gid.to_string(),
            "--native-sources",
            "harness.test",
            "--max-connections",
            "1",
        ])
        .stderr(Stdio::piped());
    fixture.child = Some(command.spawn().unwrap());
    let child = fixture.child.as_mut().unwrap();
    wait_socket(child, &socket);
    let info = fs::symlink_metadata(&socket).unwrap();
    assert_eq!(
        (info.uid(), info.gid(), info.mode() & 0o7777),
        (uid, gid, 0o660)
    );
    assert!(!transport.join("agenturm-storage-v1.sock").exists());
    let database = data.join("history.sqlite");
    let metadata = fs::symlink_metadata(&database).unwrap();
    assert_eq!((metadata.uid(), metadata.mode() & 0o7777), (uid, 0o600));
    let mut impostor = UnixStream::connect(&socket).unwrap();
    impostor
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    assert_eq!(impostor.read(&mut [0]).unwrap(), 0);
    assert!(child.wait().unwrap().success());
    assert!(!socket.exists());
    assert!(
        Store::open_read_only(&database)
            .unwrap()
            .observations()
            .unwrap()
            .is_empty()
    );

    fixture.child = Some(command.spawn().unwrap());
    let child = fixture.child.as_mut().unwrap();
    wait_socket(child, &socket);
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(socket.exists(), "SIGKILL must leave a stale endpoint");

    fixture.child = Some(command.spawn().unwrap());
    let child = fixture.child.as_mut().unwrap();
    let restart_deadline = Instant::now() + Duration::from_secs(5);
    let mut impostor = loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) && Instant::now() < restart_deadline =>
            {
                assert!(child.try_wait().unwrap().is_none(), "native restart failed");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("native restart did not reclaim its socket: {error}"),
        }
    };
    impostor
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert_eq!(impostor.read(&mut [0]).unwrap(), 0);
    assert!(child.wait().unwrap().success());
    assert!(!socket.exists());

    fs::write(&socket, b"unrelated file").unwrap();
    assert!(!command.output().unwrap().status.success());
    assert_eq!(fs::read(&socket).unwrap(), b"unrelated file");
    fs::remove_file(&socket).unwrap();
    std::os::unix::fs::symlink(&database, &socket).unwrap();
    assert!(!command.output().unwrap().status.success());
    assert!(fs::symlink_metadata(&socket).unwrap().is_symlink());
    fs::remove_file(&socket).unwrap();
    let foreign_listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
    assert!(!command.output().unwrap().status.success());
    assert!(socket.exists());
    drop(foreign_listener);
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!command.output().unwrap().status.success());
    assert!(socket.exists());
}
