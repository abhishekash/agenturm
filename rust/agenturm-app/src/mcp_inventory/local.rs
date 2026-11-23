use super::{toml, valid_name};
use crate::discovery::SourceAuthority;
use agenturm_ui::{ConfiguredMcpServer, McpInventory};
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_CONFIG: u64 = 256 * 1024;
const MAX_FILES: usize = 16;
const MAX_NAMES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    sources: Vec<ConfigSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigSource {
    client: Client,
    scope: Scope,
    path: PathBuf,
}

#[derive(Clone, Copy, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum Client {
    Codex,
    Claude,
    Cursor,
    Gemini,
    Copilot,
    Opencode,
    Vscode,
}

impl Client {
    fn title(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini CLI",
            Self::Copilot => "GitHub Copilot CLI",
            Self::Opencode => "OpenCode",
            Self::Vscode => "VS Code",
        }
    }
}

#[derive(Clone, Copy, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum Scope {
    User,
    Project,
    System,
}

impl Scope {
    fn title(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
            Self::System => "System",
        }
    }
}

pub(super) fn configured(authority: Option<&SourceAuthority>) -> Option<McpInventory> {
    if let Some(path) = std::env::var_os("AGENTURM_MCP_CONFIG") {
        let path = PathBuf::from(path);
        let manifest = read_file(&path, MAX_MANIFEST, Access::Manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<Manifest>(&raw).ok())
            .filter(|manifest| manifest.version == 1 && manifest.sources.len() <= MAX_FILES);
        return Some(manifest.map_or_else(
            || McpInventory {
                partial: true,
                ..McpInventory::default()
            },
            |manifest| read_sources(manifest.sources, false),
        ));
    }
    let authority = authority?;
    let mut paths = Vec::new();
    if let Some(home) = &authority.codex_home {
        paths.push(ConfigSource {
            client: Client::Codex,
            scope: Scope::User,
            path: home.join("config.toml"),
        });
    }
    if let Some(claude) = authority
        .follow
        .as_ref()
        .and_then(|follow| follow.claude_root())
        .filter(|root| root.file_name().is_some_and(|name| name == "projects"))
        .and_then(Path::parent)
    {
        paths.push(ConfigSource {
            client: Client::Claude,
            scope: Scope::User,
            path: claude.join(".claude.json"),
        });
        if claude.file_name().is_some_and(|name| name == ".claude")
            && let Some(home) = claude.parent()
        {
            paths.push(ConfigSource {
                client: Client::Claude,
                scope: Scope::User,
                path: home.join(".claude.json"),
            });
        }
    }
    (!paths.is_empty()).then(|| read_sources(paths, true))
}

#[derive(Clone, Copy)]
enum Access {
    Manifest,
    Config(Scope),
}

fn read_file(path: &Path, maximum: u64, access: Access) -> io::Result<String> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid MCP configuration input",
        )
    };
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir) {
        return Err(invalid());
    }
    let directories = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", directories, Mode::empty())?;
    let mut parts = parts.peekable();
    let file = loop {
        let Some(Component::Normal(name)) = parts.next() else {
            return Err(invalid());
        };
        if parts.peek().is_none() {
            break File::from(openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
        }
        directory = openat(&directory, name, directories, Mode::empty())?;
    };
    let before = file.metadata()?;
    // SAFETY: geteuid takes no arguments and has no memory preconditions.
    let uid = unsafe { libc::geteuid() };
    let owner_ok =
        before.uid() == uid || matches!(access, Access::Config(Scope::System)) && before.uid() == 0;
    let forbidden = match access {
        Access::Manifest => 0o077,
        Access::Config(_) => 0o022,
    };
    if !before.is_file()
        || !owner_ok
        || before.nlink() != 1
        || before.mode() & forbidden != 0
        || before.len() > maximum
    {
        return Err(invalid());
    }
    let mut raw = String::new();
    (&file).take(maximum + 1).read_to_string(&mut raw)?;
    let after = file.metadata()?;
    if raw.len() as u64 != before.len()
        || before.len() != after.len()
        || (
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return Err(invalid());
    }
    Ok(raw)
}

type Names = BTreeMap<String, Option<bool>>;

fn read_sources(sources: Vec<ConfigSource>, optional: bool) -> McpInventory {
    let mut inventory = McpInventory::default();
    let mut declarations = BTreeMap::new();
    for source in sources {
        let raw = match read_file(&source.path, MAX_CONFIG, Access::Config(source.scope)) {
            Ok(raw) => raw,
            Err(error) if optional && error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                inventory.partial = true;
                continue;
            }
        };
        let extension = source
            .path
            .extension()
            .and_then(|extension| extension.to_str());
        let parsed = match (source.client, extension) {
            (Client::Codex, Some("toml")) => Some(toml::declarations(&raw)),
            (client, Some("json")) => json_names(&raw, client),
            (client, Some("jsonc")) => jsonc(&raw).and_then(|source| json_names(&source, client)),
            _ => None,
        };
        let (names, partial) = parsed.unwrap_or_else(|| (Names::new(), true));
        inventory.partial |= partial;
        for (name, enabled) in names {
            let key = (source.client, source.scope, name);
            if declarations.len() >= MAX_NAMES && !declarations.contains_key(&key) {
                inventory.partial = true;
                continue;
            }
            declarations
                .entry(key)
                .and_modify(|value| {
                    if *value != enabled {
                        *value = None;
                        inventory.partial = true;
                    }
                })
                .or_insert(enabled);
        }
    }
    inventory.servers = declarations
        .into_iter()
        .map(|((client, scope, name), enabled)| ConfiguredMcpServer {
            name,
            client: client.title().into(),
            scope: scope.title().into(),
            enabled,
        })
        .collect();
    inventory
}

#[derive(Deserialize)]
struct ServerMetadata {
    enabled: Option<bool>,
}

struct Object<'a>(BTreeMap<String, &'a RawValue>);

impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueFields;

        impl<'de> Visitor<'de> for UniqueFields {
            type Value = Object<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("configuration object with distinct fields")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Self::Value, A::Error> {
                let mut rows = BTreeMap::new();
                while let Some((name, raw)) = fields.next_entry::<String, &RawValue>()? {
                    if rows.insert(name, raw).is_some() {
                        return Err(de::Error::custom("duplicate configuration field"));
                    }
                }
                Ok(Object(rows))
            }
        }

        deserializer.deserialize_map(UniqueFields)
    }
}

fn json_names(raw: &str, client: Client) -> Option<(Names, bool)> {
    let Object(root) = serde_json::from_str(raw).ok()?;
    let field = match client {
        Client::Opencode => "mcp",
        Client::Vscode => "servers",
        _ => "mcpServers",
    };
    let Some(config) = root.get(field) else {
        return Some((Names::new(), false));
    };
    let Object(mut rows) = serde_json::from_str(config.get()).ok()?;
    if client == Client::Opencode
        && let Some(servers) = rows.get("servers")
    {
        rows = serde_json::from_str::<Object<'_>>(servers.get()).ok()?.0;
    }
    let mut names = Names::new();
    let mut partial = false;
    for (name, config) in rows {
        if !valid_name(&name) || !config.get().starts_with('{') || names.len() >= MAX_NAMES {
            partial = true;
            continue;
        }
        let enabled = match serde_json::from_str::<ServerMetadata>(config.get()) {
            Ok(row) => row.enabled,
            Err(_) => {
                partial = true;
                None
            }
        };
        names.insert(name, enabled);
    }
    Some((names, partial))
}

fn jsonc(raw: &str) -> Option<String> {
    let mut normalized = Vec::with_capacity(raw.len());
    let mut input = raw.bytes().peekable();
    let mut quoted = false;
    let mut comma = None;
    while let Some(byte) = input.next() {
        if quoted {
            normalized.push(byte);
            match byte {
                b'\"' => quoted = false,
                b'\\' => normalized.push(input.next()?),
                _ => {}
            }
            continue;
        }
        if byte == b'/' {
            match input.peek() {
                Some(b'/') => {
                    input.by_ref().find(|byte| *byte == b'\n');
                    normalized.push(b'\n');
                    continue;
                }
                Some(b'*') => {
                    input.next();
                    loop {
                        if input.next()? == b'*' && input.peek() == Some(&b'/') {
                            input.next();
                            break;
                        }
                    }
                    normalized.push(b' ');
                    continue;
                }
                _ => {}
            }
        }
        if !byte.is_ascii_whitespace() {
            if matches!(byte, b'}' | b']')
                && let Some(index) = comma
            {
                normalized[index] = b' ';
            }
            comma = (byte == b',').then_some(normalized.len());
        }
        quoted = byte == b'\"';
        normalized.push(byte);
    }
    String::from_utf8(normalized).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenturm-mcp-inventory-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn selected_configuration_returns_names_and_enabled_flags_only() {
        let temporary = Temp::new();
        let toml = temporary.0.join("config.toml");
        let claude = temporary.0.join(".mcp.json");
        let open = temporary.0.join("opencode.jsonc");
        fs::write(
            &toml,
            "[mcp_servers.shared]\ncommand = 'secret-program'\nenabled = false\n",
        )
        .unwrap();
        fs::write(&claude, r#"{"mcpServers":{"shared":{"url":"https://example.invalid/credential","headers":{"Authorization":"secret"}}}}"#).unwrap();
        fs::write(
            &open,
            r#"{
            // Supported OpenCode shape
            "mcp": { "servers": { "idle": {"url":"https://invalid.test/#secret",},},},
        }"#,
        )
        .unwrap();
        for input in [&toml, &claude, &open] {
            fs::set_permissions(input, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let inventory = read_sources(
            vec![
                ConfigSource {
                    client: Client::Codex,
                    scope: Scope::User,
                    path: toml,
                },
                ConfigSource {
                    client: Client::Claude,
                    scope: Scope::Project,
                    path: claude,
                },
                ConfigSource {
                    client: Client::Opencode,
                    scope: Scope::User,
                    path: open,
                },
            ],
            false,
        );
        assert!(!inventory.partial, "fixture inventory was partial: {inventory:?}");
        assert_eq!(
            inventory.servers,
            vec![
                ConfiguredMcpServer {
                    name: "shared".into(),
                    client: "Codex".into(),
                    scope: "User".into(),
                    enabled: Some(false)
                },
                ConfiguredMcpServer {
                    name: "shared".into(),
                    client: "Claude Code".into(),
                    scope: "Project".into(),
                    enabled: None
                },
                ConfiguredMcpServer {
                    name: "idle".into(),
                    client: "OpenCode".into(),
                    scope: "User".into(),
                    enabled: None
                },
            ]
        );
        assert!(!format!("{inventory:?}").contains("secret"));
        assert!(
            json_names(
                r#"{"mcpServers":{"x":{},"x":{"enabled":false}}}"#,
                Client::Claude
            )
            .is_none()
        );
        let (names, partial) = json_names(
            r#"{"mcpServers":{"x":{"enabled": false, "enabled":true}}}"#,
            Client::Claude,
        )
        .unwrap();
        assert_eq!(names, Names::from([("x".into(), None)]));
        assert!(partial);
    }

    #[test]
    fn config_authority_denies_symlinks_and_loose_manifest_permissions() {
        let scratch = Temp::new();
        let target = scratch.0.join("actual.json");
        let link = scratch.0.join("link.json");
        fs::write(&target, r#"{"mcpServers":{"visible":{}}}"#).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&target, &link).unwrap();
        assert!(read_file(&target, MAX_CONFIG, Access::Config(Scope::User)).is_ok());
        assert!(read_file(&link, MAX_CONFIG, Access::Config(Scope::User)).is_err());
        assert!(read_file(&target, MAX_MANIFEST, Access::Manifest).is_err());
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_file(&target, MAX_MANIFEST, Access::Manifest).is_ok());
    }
}
