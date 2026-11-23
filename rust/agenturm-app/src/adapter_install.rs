//! Manages locally installed trusted adapters in an explicit private catalog.

use super::{
    MAX_PLUGIN_CATALOG_BYTES, decode_adapter_catalog, valid_config_path, valid_plugin_feature,
};
use agenturm_kernel::ExtensionId;
use agenturm_runtime::ObservationFormat;
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, RenameFlags, fchmod, flock, mkdirat, open, openat,
    renameat, renameat_with, unlinkat,
};
use rustix::io::Errno;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const USAGE: &str = concat!(
    "Usage: agenturm adapter install --manifest <local-json> --sha256 <expected> --root <private-directory> [--replace-sha256 <current>]\n",
    "       agenturm adapter list --root <private-directory> [--json]\n",
    "       agenturm adapter show --root <private-directory> --plugin-id <id> [--json]\n",
    "       agenturm adapter remove --root <private-directory> --plugin-id <id> --sha256 <current> [--json]",
);
const CATALOG: &str = "catalog.json";
const ARTIFACTS: &str = "artifacts";
const MAX_ARTIFACT: u64 = 256 * 1024 * 1024;
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
const PRIVATE_DIR: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);
const PRIVATE_EXEC: Mode = Mode::RUSR.union(Mode::XUSR);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Options {
    Help,
    Install(InstallOptions),
    List {
        root: PathBuf,
        json: bool,
    },
    Show {
        root: PathBuf,
        plugin_id: ExtensionId,
        json: bool,
    },
    Remove {
        root: PathBuf,
        plugin_id: ExtensionId,
        sha256: String,
        json: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallOptions {
    manifest: PathBuf,
    sha256: String,
    root: PathBuf,
    replace: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    plugin_id: String,
    platform: String,
    adapter: PathBuf,
    sha256: String,
    protocol: u32,
    features: Vec<String>,
    observation_format: String,
}

pub(super) fn parse(args: &[String]) -> Result<Options, String> {
    let [action, options @ ..] = args else {
        return Err(USAGE.into());
    };
    match action.as_str() {
        "--help" | "-h" | "help" if options.is_empty() => Ok(Options::Help),
        "install" => parse_install(options).map(Options::Install),
        "list" | "show" | "remove" => parse_management(action, options),
        _ => Err(USAGE.into()),
    }
}

fn parse_management(action: &str, options: &[String]) -> Result<Options, String> {
    let (mut root, mut plugin_id, mut sha256, mut json) = (None, None, None, false);
    let mut args = options.iter();
    while let Some(key) = args.next() {
        if key == "--json" {
            if json {
                return Err(USAGE.into());
            }
            json = true;
            continue;
        }
        let slot = match key.as_str() {
            "--root" => &mut root,
            "--plugin-id" if action != "list" => &mut plugin_id,
            "--sha256" if action == "remove" => &mut sha256,
            _ => return Err(USAGE.into()),
        };
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or(USAGE)?;
        if slot.replace(value.clone()).is_some() {
            return Err(USAGE.into());
        }
    }
    let root = PathBuf::from(root.ok_or(USAGE)?);
    if action == "list" {
        return Ok(Options::List { root, json });
    }
    let plugin_id =
        ExtensionId::parse(plugin_id.ok_or(USAGE)?).map_err(|_| "Invalid adapter plugin ID")?;
    if action == "show" {
        return Ok(Options::Show {
            root,
            plugin_id,
            json,
        });
    }
    let sha256 = sha256.ok_or(USAGE)?;
    if !valid_digest(&sha256) {
        return Err("Expected SHA-256 must be 64 lower-case hexadecimal characters".into());
    }
    Ok(Options::Remove {
        root,
        plugin_id,
        sha256,
        json,
    })
}

fn parse_install(options: &[String]) -> Result<InstallOptions, String> {
    if !matches!(options.len(), 6 | 8) {
        return Err(USAGE.into());
    }
    let (mut manifest, mut sha256, mut root, mut replace) = (None, None, None, None);
    for [key, value] in options.as_chunks::<2>().0 {
        let slot = match key.as_str() {
            "--manifest" => &mut manifest,
            "--sha256" => &mut sha256,
            "--root" => &mut root,
            "--replace-sha256" => &mut replace,
            _ => return Err(USAGE.into()),
        };
        if value.is_empty() || value.starts_with("--") || slot.replace(value.clone()).is_some() {
            return Err(USAGE.into());
        }
    }
    Ok(InstallOptions {
        manifest: PathBuf::from(manifest.ok_or(USAGE)?),
        sha256: sha256.ok_or(USAGE)?,
        root: PathBuf::from(root.ok_or(USAGE)?),
        replace,
    })
}

pub fn execute(options: &Options) -> Result<String, String> {
    let (path, json) = match options {
        Options::Help => return Ok(USAGE.into()),
        Options::Install(options) => return install(options),
        Options::List { root, json }
        | Options::Show { root, json, .. }
        | Options::Remove { root, json, .. } => (absolute(root)?, *json),
    };
    let (parent, name) = open_parent(&path)?;
    let root = open_private_directory(&parent, name)?;
    let _lock = lock_catalog(&root, false)?;
    let artifacts = open_private_directory(&root, OsStr::new(ARTIFACTS))?;
    let mut plugins = existing_plugins(&root, &artifacts)?;
    let catalog = catalog_result(&root, &path)?;
    match options {
        Options::List { .. } if json => to_json(&json!({"catalog": catalog, "plugins": plugins})),
        Options::List { .. } => {
            if plugins.is_empty() {
                return Ok("No adapters registered".into());
            }
            let mut lines = vec!["Plugin  SHA-256".into()];
            lines.extend(
                plugins
                    .iter()
                    .map(|entry| format!("{}  {}", entry.plugin_id, entry.sha256)),
            );
            Ok(lines.join("\n"))
        }
        Options::Show { plugin_id, .. } => {
            let entry = &plugins[plugin_index(&plugins, plugin_id)?];
            if json {
                return to_json(entry);
            }
            Ok(format!(
                "Plugin  {}\nSHA-256  {}\nArtifact  {}\nProtocol  {}\nObservation format  {}\nFeatures  {}",
                entry.plugin_id,
                entry.sha256,
                path.join(&entry.adapter).display(),
                entry.protocol,
                entry.observation_format,
                entry.features.join(", "),
            ))
        }
        Options::Remove {
            plugin_id, sha256, ..
        } => {
            let index = plugin_index(&plugins, plugin_id)?;
            let entry = &plugins[index];
            if entry.sha256 != *sha256 {
                return Err(
                    "Current adapter SHA-256 does not match; inspect the selected plugin".into(),
                );
            }
            plugins.remove(index);
            publish_catalog(&root, &path, &encode_catalog(&plugins)?)?;
            if json {
                return to_json(
                    &json!({"catalog": catalog, "plugin_id": plugin_id, "sha256": sha256}),
                );
            }
            Ok(format!(
                "Unregistered {} from {catalog}",
                plugin_id.as_str()
            ))
        }
        Options::Help | Options::Install(_) => {
            unreachable!("handled before opening the managed catalog")
        }
    }
}

fn plugin_index(plugins: &[ManagedEntry], plugin_id: &ExtensionId) -> Result<usize, String> {
    plugins
        .iter()
        .position(|entry| entry.plugin_id == plugin_id.as_str())
        .ok_or_else(|| format!("Adapter plugin {} is not registered", plugin_id.as_str()))
}

fn to_json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "Cannot encode adapter catalog output".into())
}

fn install(options: &InstallOptions) -> Result<String, String> {
    if !valid_digest(&options.sha256)
        || options
            .replace
            .as_deref()
            .is_some_and(|pin| !valid_digest(pin))
    {
        return Err("Expected SHA-256 must be 64 lower-case hexadecimal characters".into());
    }
    let manifest_path = absolute(&options.manifest)?;
    let root_path = absolute(&options.root)?;
    if !valid_config_path(&root_path.join(ARTIFACTS).join(&options.sha256)) {
        return Err("Managed adapter artifact path exceeds the catalog limit".into());
    }
    let (package, manifest_filename) = open_parent(&manifest_path)?;
    let manifest_file = read_member(&package, manifest_filename)?;
    let manifest: Manifest = serde_json::from_slice(&read_bounded(manifest_file)?)
        .map_err(|_| "Invalid adapter package manifest")?;
    validate_manifest(&manifest, &options.sha256)?;
    let mut source = open_relative_member(package, &manifest.adapter)?;
    let source_metadata = source
        .metadata()
        .map_err(|_| "Cannot inspect adapter executable")?;
    if !source_metadata.is_file()
        || source_metadata.mode() & 0o111 == 0
        || source_metadata.len() > MAX_ARTIFACT
    {
        return Err("Adapter must be a bounded regular executable".into());
    }

    let (parent, name) = open_parent(&root_path)?;
    let root = private_directory(&parent, name)?;
    let _lock = lock_catalog(&root, true)?;
    let artifacts = private_directory(&root, OsStr::new(ARTIFACTS))?;
    let mut plugins = existing_plugins(&root, &artifacts)?;
    let new_entry = catalog_entry(
        &manifest.plugin_id,
        &manifest.sha256,
        &manifest.features,
        &manifest.observation_format,
    );
    if let Some(existing) = plugins
        .iter_mut()
        .find(|entry| entry.plugin_id == manifest.plugin_id)
    {
        if existing == &new_entry {
            // Check the input even on an exact retry. The previously installed
            // bytes alone do not prove the newly supplied package.
            verify_copy(&mut source, &mut std::io::sink(), &manifest.sha256)?;
            return catalog_result(&root, &root_path);
        }
        if options.replace.is_none() || options.replace.as_deref() != Some(existing.sha256.as_str())
        {
            return Err("Adapter plugin ID already selects a different package".into());
        }
        *existing = new_entry;
    } else {
        if options.replace.is_some() {
            return Err("Cannot replace a plugin that is absent from the managed catalog".into());
        }
        if plugins.len() >= 64 {
            return Err("Adapter catalog capacity is exhausted".into());
        }
        plugins.push(new_entry);
    }
    plugins.sort_unstable_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    let catalog = encode_catalog(&plugins)?;

    let mut artifact_temp = StagedFile::new(&artifacts)?;
    verify_copy(&mut source, &mut artifact_temp.file, &manifest.sha256)?;
    fchmod(&artifact_temp.file, PRIVATE_EXEC).map_err(|_| "Cannot set private executable mode")?;
    artifact_temp
        .file
        .sync_all()
        .map_err(|_| "Cannot synchronize adapter artifact")?;
    match renameat_with(
        &artifacts,
        &artifact_temp.name,
        &artifacts,
        &manifest.sha256,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {}
        Err(Errno::EXIST) => {
            let mut installed = read_member(&artifacts, OsStr::new(&manifest.sha256))?;
            verify_private_file(&installed, PRIVATE_EXEC)?;
            verify_copy(&mut installed, &mut std::io::sink(), &manifest.sha256)?;
        }
        Err(_) => return Err("Cannot publish adapter artifact".into()),
    }
    artifacts
        .sync_all()
        .map_err(|_| "Cannot synchronize adapter artifact directory")?;

    publish_catalog(&root, &root_path, &catalog)
}

fn encode_catalog(plugins: &[ManagedEntry]) -> Result<Vec<u8>, String> {
    let catalog = serde_json::to_vec_pretty(&json!({"version":1,"plugins":plugins}))
        .map_err(|_| "Cannot encode adapter catalog")?;
    if catalog.len() as u64 > MAX_PLUGIN_CATALOG_BYTES {
        return Err("Adapter catalog exceeds 64 KiB".into());
    }
    Ok(catalog)
}

fn publish_catalog(root: &File, path: &Path, catalog: &[u8]) -> Result<String, String> {
    let mut catalog_temp = StagedFile::new(root)?;
    catalog_temp
        .file
        .write_all(catalog)
        .map_err(|_| "Cannot write adapter catalog")?;
    catalog_temp
        .file
        .sync_all()
        .map_err(|_| "Cannot synchronize adapter catalog")?;
    // Verify the selected root again before atomically publishing a complete catalog.
    catalog_result(root, path)?;
    renameat(root, &catalog_temp.name, root, CATALOG)
        .map_err(|_| "Cannot publish adapter catalog")?;
    root.sync_all()
        .map_err(|_| "Cannot synchronize adapter catalog directory")?;
    catalog_result(root, path)
}

fn validate_manifest(manifest: &Manifest, expected: &str) -> Result<(), String> {
    if manifest.version != 1
        || manifest.protocol != 1
        || manifest.platform != format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    {
        return Err("Unsupported adapter package version, protocol, or platform".into());
    }
    manifest
        .observation_format
        .parse::<ObservationFormat>()
        .map_err(|_| "Unsupported adapter observation format")?;
    ExtensionId::parse(&manifest.plugin_id).map_err(|_| "Invalid adapter plugin ID")?;
    if manifest.sha256 != expected {
        return Err("Adapter manifest does not match the operator's SHA-256 pin".into());
    }
    if !valid_config_path(&manifest.adapter)
        || !manifest
            .adapter
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || manifest.features.len() > 64
        || manifest
            .features
            .iter()
            .any(|feature| !valid_plugin_feature(feature))
        || manifest.features.iter().collect::<BTreeSet<_>>().len() != manifest.features.len()
        || !manifest
            .features
            .iter()
            .any(|feature| feature == "source-adapter/1")
        || !manifest
            .features
            .iter()
            .any(|feature| feature == "configured-root/1")
    {
        return Err("Invalid local adapter path or source protocol features".into());
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    let path = std::path::absolute(path).map_err(|_| "Cannot resolve adapter installation path")?;
    if !valid_config_path(&path)
        || path.file_name().is_none()
        || path
            .to_str()
            .is_none_or(|text| text.chars().any(char::is_control))
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err("Adapter installation paths must be bounded and avoid parent traversal".into());
    }
    Ok(path)
}

fn open_parent(path: &Path) -> Result<(File, &OsStr), String> {
    let parent = path
        .parent()
        .ok_or("Adapter installation path needs a parent directory")?;
    let name = path
        .file_name()
        .ok_or("Adapter installation path needs a file name")?;
    let mut directory = File::from(
        open("/", DIRECTORY, Mode::empty())
            .map_err(|_| "Cannot open adapter installation parent")?,
    );
    for component in parent.components().skip(1) {
        let Component::Normal(name) = component else {
            return Err("Invalid adapter installation parent path".into());
        };
        directory = File::from(
            openat(&directory, name, DIRECTORY, Mode::empty())
                .map_err(|_| "Cannot open adapter installation parent without following links")?,
        );
    }
    Ok((directory, name))
}

fn open_relative_member(mut directory: File, path: &Path) -> Result<File, String> {
    let mut names = path.components().peekable();
    while let Some(Component::Normal(name)) = names.next() {
        if names.peek().is_none() {
            return read_member(&directory, name);
        }
        directory = File::from(
            openat(&directory, name, DIRECTORY, Mode::empty())
                .map_err(|_| "Cannot open adapter package directory without following links")?,
        );
    }
    Err("Invalid adapter package executable path".into())
}

fn read_member(directory: &File, name: &OsStr) -> Result<File, String> {
    openat(directory, name, READ, Mode::empty())
        .map(File::from)
        .map_err(|_| "Cannot open adapter file without following links".into())
}

fn read_bounded(file: File) -> Result<Vec<u8>, String> {
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect adapter manifest")?;
    if !metadata.is_file() || metadata.len() > MAX_PLUGIN_CATALOG_BYTES {
        return Err("Adapter manifest must be a regular file under 64 KiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLUGIN_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read adapter manifest")?;
    if bytes.len() as u64 > MAX_PLUGIN_CATALOG_BYTES {
        return Err("Adapter manifest exceeds 64 KiB".into());
    }
    Ok(bytes)
}

fn private_directory(parent: &File, name: &OsStr) -> Result<File, String> {
    match mkdirat(parent, name, PRIVATE_DIR) {
        Ok(()) => parent
            .sync_all()
            .map_err(|_| "Cannot synchronize private adapter directory")?,
        Err(Errno::EXIST) => {}
        Err(_) => return Err("Cannot create private adapter directory".into()),
    }
    open_private_directory(parent, name)
}

fn open_private_directory(parent: &File, name: &OsStr) -> Result<File, String> {
    let directory = File::from(
        openat(parent, name, DIRECTORY, Mode::empty())
            .map_err(|_| "Cannot open private adapter directory without following links")?,
    );
    let metadata = directory
        .metadata()
        .map_err(|_| "Cannot inspect private adapter directory")?;
    // SAFETY: geteuid takes no arguments and writes to no memory.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
        return Err(
            "Managed adapter directories must be owned by the current user and mode 0700".into(),
        );
    }
    Ok(directory)
}

fn verify_private_file(file: &File, mode: Mode) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect managed adapter file")?;
    // SAFETY: geteuid takes no arguments and writes to no memory.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != mode.bits()
    {
        return Err("Managed adapter file is not a private regular file".into());
    }
    Ok(())
}

fn lock_catalog(root: &File, create: bool) -> Result<CatalogLock, String> {
    let mut flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    if create {
        flags |= OFlags::CREATE;
    }
    let lock = File::from(
        openat(root, ".catalog.lock", flags, PRIVATE_FILE)
            .map_err(|_| "Cannot open managed adapter catalog lock")?,
    );
    verify_private_file(&lock, PRIVATE_FILE)?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| "Managed adapter catalog is busy")?;
    Ok(CatalogLock(lock))
}

struct CatalogLock(File);

impl Drop for CatalogLock {
    fn drop(&mut self) {
        // A concurrent fork can hold a copy until exec closes CLOEXEC FDs.
        // Unlock the shared file description immediately.
        let _ = flock(&self.0, FlockOperation::Unlock);
    }
}

fn existing_plugins(root: &File, artifacts: &File) -> Result<Vec<ManagedEntry>, String> {
    let catalog = match openat(root, CATALOG, READ, Mode::empty()) {
        Ok(file) => File::from(file),
        Err(Errno::NOENT) => return Ok(Vec::new()),
        Err(_) => return Err("Cannot open managed adapter catalog".into()),
    };
    verify_private_file(&catalog, PRIVATE_FILE)?;
    let root_fd = PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd()));
    let mut entries = Vec::new();
    for (plugin, entry) in decode_adapter_catalog(catalog, &root_fd.join(CATALOG), true)? {
        let installed = read_member(artifacts, OsStr::new(&entry.sha256))?;
        verify_private_file(&installed, PRIVATE_EXEC)?;
        let path = root_fd.join(ARTIFACTS).join(&entry.sha256);
        if path
            .canonicalize()
            .map_err(|_| "Cannot locate managed adapter artifact")?
            != entry.adapter
        {
            return Err("Managed adapter catalog contains an external artifact".into());
        }
        entries.push(catalog_entry(
            &plugin,
            &entry.sha256,
            &entry.features,
            &entry.observation_format.to_string(),
        ));
    }
    Ok(entries)
}

#[derive(serde::Serialize, PartialEq, Eq)]
struct ManagedEntry {
    plugin_id: String,
    adapter: String,
    sha256: String,
    protocol: u32,
    features: Vec<String>,
    observation_format: String,
}

fn catalog_entry(plugin_id: &str, sha256: &str, features: &[String], format: &str) -> ManagedEntry {
    ManagedEntry {
        plugin_id: plugin_id.into(),
        adapter: format!("{ARTIFACTS}/{sha256}"),
        sha256: sha256.into(),
        protocol: 1,
        features: features.to_vec(),
        observation_format: format.into(),
    }
}

fn verify_copy(
    source: &mut impl Read,
    destination: &mut impl Write,
    expected: &str,
) -> Result<(), String> {
    let mut source = source.take(MAX_ARTIFACT + 1);
    let mut digest = Sha256::new();
    let mut remaining = MAX_ARTIFACT;
    let mut buffer = [0; 64 * 1024];
    loop {
        let length = source
            .read(&mut buffer)
            .map_err(|_| "Cannot read adapter executable")?;
        if length == 0 {
            break;
        }
        remaining = remaining
            .checked_sub(length as u64)
            .ok_or("Adapter executable exceeds 256 MiB")?;
        digest.update(&buffer[..length]);
        destination
            .write_all(&buffer[..length])
            .map_err(|_| "Cannot stage adapter executable")?;
    }
    if remaining == MAX_ARTIFACT || format!("{:x}", digest.finalize()) != expected {
        return Err("Adapter executable bytes do not match the expected SHA-256".into());
    }
    Ok(())
}

fn catalog_result(root: &File, path: &Path) -> Result<String, String> {
    let (parent, name) = open_parent(path)?;
    let now = File::from(
        openat(&parent, name, DIRECTORY, Mode::empty())
            .map_err(|_| "Managed adapter directory changed during the operation")?,
    )
    .metadata()
    .map_err(|_| "Managed adapter directory changed during the operation")?;
    let original = root
        .metadata()
        .map_err(|_| "Cannot inspect managed adapter directory")?;
    if now.dev() != original.dev() || now.ino() != original.ino() {
        return Err("Managed adapter directory changed during the operation".into());
    }
    Ok(path.join(CATALOG).display().to_string())
}

struct StagedFile<'a> {
    directory: &'a File,
    name: String,
    file: File,
}

impl<'a> StagedFile<'a> {
    fn new(directory: &'a File) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..8 {
            let name = format!(
                ".install-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            match openat(
                directory,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                PRIVATE_FILE,
            ) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        name,
                        file: File::from(file),
                    });
                }
                Err(Errno::EXIST) => {}
                Err(_) => break,
            }
        }
        Err("Cannot create temporary adapter installation file".into())
    }
}

impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        let _ = unlinkat(self.directory, &self.name, AtFlags::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_adapter_catalog;
    use serde_json::Value;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        directory: PathBuf,
        options: InstallOptions,
        manifest: Value,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "agenturm-adapter-install-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let source = directory.join("adapter");
            let bytes = format!(
                "#!/bin/sh\nprintf started > '{}'\n",
                directory.join("unexpected-child").display()
            );
            fs::write(&source, bytes.as_bytes()).unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
            let options = InstallOptions {
                manifest: directory.join("manifest.json"),
                sha256: format!("{:x}", Sha256::digest(bytes.as_bytes())),
                root: directory.join("managed"),
                replace: None,
            };
            let manifest = json!({
                "version":1,"adapter":"adapter","sha256":options.sha256,
                "plugin_id":"fixture.local","protocol":1,
                "platform":format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                "features":["source-adapter/1","configured-root/1"],
                "observation_format":"agenturm-observations/1",
            });
            let fixture = Self {
                directory,
                options,
                manifest,
                source,
            };
            fixture.write_manifest(&fixture.manifest);
            fixture
        }

        fn write_manifest(&self, value: &Value) {
            fs::write(&self.options.manifest, serde_json::to_vec(value).unwrap()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn lifecycle(fixture: &Fixture, arguments: &[&str]) -> Result<String, String> {
        let mut command = vec!["adapter".into()];
        command.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        command.extend([
            "--root".into(),
            fixture.options.root.to_str().unwrap().into(),
        ]);
        let crate::Command::InstallAdapter(options) = crate::parse_command(&command)? else {
            panic!("expected managed adapter command");
        };
        execute(&options)
    }

    #[test]
    fn lifecycle_registrations_preserve_artifacts_and_other_plugins() {
        let fixture = Fixture::new();
        assert!(lifecycle(&fixture, &["list", "--json"]).is_err());
        assert!(!fixture.options.root.exists());
        install(&fixture.options).unwrap();
        let shared = fixture
            .options
            .root
            .join(ARTIFACTS)
            .join(&fixture.options.sha256);
        let original_pin = fixture.options.sha256.as_str();
        let original_artifact = fs::read(&shared).unwrap();
        let catalog = fixture.options.root.join(CATALOG);
        let sidecar = fixture.options.root.join("history.sqlite");
        fs::write(&sidecar, b"retained history and grants").unwrap();

        let mut other = fixture.manifest.clone();
        other["plugin_id"] = json!("fixture.other");
        fixture.write_manifest(&other);
        install(&fixture.options).unwrap();
        let before = fs::read(&catalog).unwrap();
        let listed: Value =
            serde_json::from_str(&lifecycle(&fixture, &["list", "--json"]).unwrap()).unwrap();
        let reported = listed["plugins"].as_array().unwrap();
        let ids: Vec<_> = reported
            .iter()
            .map(|entry| entry["plugin_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["fixture.local", "fixture.other"]);
        let shown: Value = serde_json::from_str(
            &lifecycle(
                &fixture,
                &["show", "--plugin-id", "fixture.local", "--json"],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(shown, reported[0]);
        assert_eq!(shown["sha256"], original_pin);
        assert_eq!(shown["adapter"], format!("artifacts/{original_pin}"));
        assert_eq!(fs::read(&catalog).unwrap(), before);
        assert!(lifecycle(&fixture, &["remove", "--plugin-id", "fixture.local"]).is_err());

        // A held installation lock and an invalid pin both leave the exact
        // validated catalog intact, even with a shared artifact.
        let root = File::open(&fixture.options.root).unwrap();
        let lock = lock_catalog(&root, false).unwrap();
        let remove = |id: &str, pin: &str| {
            lifecycle(
                &fixture,
                &["remove", "--plugin-id", id, "--sha256", pin, "--json"],
            )
        };
        assert!(
            remove("fixture.local", original_pin)
                .unwrap_err()
                .contains("busy")
        );
        assert_eq!(fs::read(&catalog).unwrap(), before);
        drop(lock);
        assert!(
            remove("fixture.local", &"0".repeat(64))
                .unwrap_err()
                .contains("does not match")
        );
        assert_eq!(fs::read(&catalog).unwrap(), before);
        let replacement_source = b"#!/bin/sh\nexit 0\n";
        let mut replacement_manifest = fixture.manifest.clone();
        let replacement = InstallOptions {
            sha256: format!("{:x}", Sha256::digest(replacement_source)),
            replace: Some(original_pin.into()),
            ..fixture.options.clone()
        };
        fs::write(&fixture.source, replacement_source).unwrap();
        replacement_manifest["sha256"] = json!(replacement.sha256);
        fixture.write_manifest(&replacement_manifest);
        install(&replacement).unwrap();
        let replaced_catalog = fs::read(&catalog).unwrap();
        assert!(
            remove("fixture.local", original_pin)
                .unwrap_err()
                .contains("does not match")
        );
        assert_eq!(fs::read(&catalog).unwrap(), replaced_catalog);
        let removed: Value =
            serde_json::from_str(&remove("fixture.local", &replacement.sha256).unwrap()).unwrap();
        assert_eq!(removed["plugin_id"], "fixture.local");
        assert_eq!(removed["sha256"], replacement.sha256);
        assert_eq!(
            load_adapter_catalog(&catalog).unwrap()["fixture.other"].sha256,
            original_pin
        );
        assert!(lifecycle(&fixture, &["show", "--plugin-id", "fixture.local"]).is_err());
        assert!(remove("fixture.local", original_pin).is_err());
        remove("fixture.other", original_pin).unwrap();
        let empty: Value = serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap();
        assert_eq!(empty, json!({"version":1,"plugins":[]}));
        assert_eq!(
            lifecycle(&fixture, &["list"]).unwrap(),
            "No adapters registered"
        );
        assert!(load_adapter_catalog(&catalog).is_err());
        assert_eq!(fs::read(&shared).unwrap(), original_artifact);
        assert_eq!(fs::read(&sidecar).unwrap(), b"retained history and grants");
        assert!(!fixture.directory.join("unexpected-child").exists());

        fs::write(&fixture.source, original_artifact).unwrap();
        fixture.write_manifest(&fixture.manifest);
        install(&fixture.options).unwrap();
        assert!(
            load_adapter_catalog(&catalog)
                .unwrap()
                .contains_key("fixture.local")
        );
    }

    #[test]
    fn publication_refuses_a_replaced_root_without_changing_either_catalog() {
        let fixture = Fixture::new();
        install(&fixture.options).unwrap();
        let root = File::open(&fixture.options.root).unwrap();
        let _lock = lock_catalog(&root, false).unwrap();
        let original = fs::read(fixture.options.root.join(CATALOG)).unwrap();
        let relocated = fixture.directory.join("original-root");
        fs::rename(&fixture.options.root, &relocated).unwrap();
        fs::create_dir(&fixture.options.root).unwrap();
        let unrelated = fixture.options.root.join(CATALOG);
        fs::write(&unrelated, b"unrelated catalog").unwrap();
        assert!(
            publish_catalog(&root, &fixture.options.root, &encode_catalog(&[]).unwrap())
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(fs::read(relocated.join(CATALOG)).unwrap(), original);
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated catalog");
        assert!(fs::read_dir(relocated).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .starts_with(".install-")
        }));
    }

    #[test]
    fn validates_manifest_and_refuses_links_and_unsafe_destinations_before_install() {
        let mut fixture = Fixture::new();
        let original = fixture.manifest.clone();
        for (field, wrong) in [
            ("observation_format", json!("other/1")),
            ("platform", json!("unsupported-platform")),
            ("protocol", json!(2)),
            ("adapter", json!("../adapter")),
            ("features", json!(["source-adapter/1"])),
            ("sha256", json!("0".repeat(64))),
        ] {
            fixture.manifest = original.clone();
            fixture.manifest[field] = wrong;
            fixture.write_manifest(&fixture.manifest);
            assert!(install(&fixture.options).is_err(), "{field}");
            assert!(!fixture.options.root.exists(), "{field}");
        }
        let link = fixture.directory.join("linked-adapter");
        symlink(&fixture.source, link).unwrap();
        fixture.manifest = original.clone();
        fixture.manifest["adapter"] = json!("linked-adapter");
        fixture.write_manifest(&fixture.manifest);
        assert!(install(&fixture.options).is_err());
        assert!(!fixture.options.root.exists());
        fixture.write_manifest(&original);
        fs::create_dir(&fixture.options.root).unwrap();
        fs::set_permissions(&fixture.options.root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(install(&fixture.options).unwrap_err().contains("mode 0700"));
        assert!(!fixture.options.root.join(CATALOG).exists());
    }

    #[test]
    fn installation_is_idempotent_and_failed_publications_preserve_current_catalog() {
        let mut fixture = Fixture::new();
        let catalog = fixture.options.root.join(CATALOG);
        assert_eq!(
            install(&fixture.options).unwrap(),
            catalog.display().to_string()
        );
        assert_eq!(
            install(&fixture.options).unwrap(),
            catalog.display().to_string()
        );
        let original = fs::read(&catalog).unwrap();
        let plugin = load_adapter_catalog(&catalog)
            .unwrap()
            .remove("fixture.local")
            .unwrap();
        assert_eq!(
            plugin.adapter,
            fixture
                .options
                .root
                .join(ARTIFACTS)
                .join(&fixture.options.sha256)
        );
        assert_eq!(fs::metadata(&plugin.adapter).unwrap().mode() & 0o777, 0o500);
        assert_eq!(fs::metadata(&catalog).unwrap().mode() & 0o777, 0o600);
        assert!(!fixture.directory.join("unexpected-child").exists());

        fs::write(&fixture.source, b"changed").unwrap();
        assert!(install(&fixture.options).unwrap_err().contains("bytes"));
        fixture.options.sha256 = format!("{:x}", Sha256::digest(b"changed"));
        fixture.manifest["sha256"] = json!(fixture.options.sha256);
        fixture.write_manifest(&fixture.manifest);
        assert!(install(&fixture.options).unwrap_err().contains("plugin ID"));
        assert!(
            !fixture
                .options
                .root
                .join(ARTIFACTS)
                .join(&fixture.options.sha256)
                .exists()
        );

        let root = File::open(&fixture.options.root).unwrap();
        let lock = lock_catalog(&root, false).unwrap();
        let mut unpublished = StagedFile::new(&root).unwrap();
        unpublished.file.write_all(b"partial catalog").unwrap();
        unpublished.file.sync_all().unwrap();
        assert!(install(&fixture.options).unwrap_err().contains("busy"));
        assert_eq!(fs::read(&catalog).unwrap(), original);
        let inherited_descriptor = lock.0.try_clone().unwrap();
        drop(unpublished);
        drop(lock);
        assert_eq!(fs::read(&catalog).unwrap(), original);

        fixture.options.replace = Some(plugin.sha256.clone());
        assert_eq!(
            install(&fixture.options).unwrap(),
            catalog.display().to_string()
        );
        assert_eq!(
            load_adapter_catalog(&catalog).unwrap()["fixture.local"].sha256,
            fixture.options.sha256
        );
        assert!(
            plugin.adapter.is_file(),
            "old artifact must remain available to running collectors"
        );
        drop(inherited_descriptor);
    }
}
