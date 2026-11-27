#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = start() {
        eprintln!("Agenturm storage service unavailable: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn start() -> std::io::Result<()> {
    use agenturm_storage_service::{ClientIdentity, NativeRolesConfig, ServiceConfig, run};
    use std::collections::{BTreeSet, HashMap};
    use std::io::{self, ErrorKind};
    use std::path::PathBuf;

    fn invalid() -> io::Error {
        io::Error::new(
            ErrorKind::InvalidInput,
            "expected --data-root PATH --transport-dir PATH --receiver-id ID --client-uid UID --client-gid GID [--max-connections N] [--native-sources ID[,ID] [--native-telemetry-payload-sources ID[=NATIVE][,ID[=NATIVE]]] [--native-reader UID:GID [--native-content-config PATH]] [--native-grant-admin UID:GID] [--native-maintenance UID:GID [--native-backup-root PATH [--native-restore NAME]]] | --experimental-v2-scratch enabled | --experimental-real-clone-activation enabled]",
        )
    }
    let mut args = std::env::args().skip(1);
    let mut values = HashMap::new();
    while let Some(key) = args.next() {
        let value = args.next().ok_or_else(invalid)?;
        if !key.starts_with("--") || values.insert(key, value).is_some() {
            return Err(invalid());
        }
    }
    if values.keys().any(|key| {
        !matches!(
            key.as_str(),
            "--data-root"
                | "--transport-dir"
                | "--receiver-id"
                | "--client-uid"
                | "--client-gid"
                | "--max-connections"
                | "--experimental-v2-scratch"
                | "--experimental-real-clone-activation"
                | "--native-sources"
                | "--native-telemetry-payload-sources"
                | "--native-reader"
                | "--native-grant-admin"
                | "--native-maintenance"
                | "--native-backup-root"
                | "--native-restore"
                | "--native-content-config"
        )
    }) {
        return Err(invalid());
    }
    let mut native_sources = BTreeSet::new();
    if let Some(sources) = values.remove("--native-sources") {
        for source in sources.split(',') {
            let id = agenturm_kernel::ExtensionId::parse(source).map_err(|_| invalid())?;
            if !native_sources.insert(id) || native_sources.len() > 64 {
                return Err(invalid());
            }
        }
    }
    let mut native_telemetry_bindings = Vec::new();
    if let Some(bindings) = values.remove("--native-telemetry-payload-sources") {
        for binding in bindings.split(',') {
            let (source, native) = binding
                .split_once('=')
                .map_or((binding, None), |(source, native)| (source, Some(native)));
            let parse = |id| agenturm_kernel::ExtensionId::parse(id).map_err(|_| invalid());
            native_telemetry_bindings.push(agenturm_store::TelemetryBinding {
                source_id: parse(source)?,
                native_source_id: native.map(parse).transpose()?,
            });
            if native_telemetry_bindings.len() > 64 {
                return Err(invalid());
            }
        }
    }
    let mut identity = |key: &str| -> io::Result<Option<ClientIdentity>> {
        values
            .remove(key)
            .map(|value| {
                let (uid, gid) = value.split_once(':').ok_or_else(invalid)?;
                Ok(ClientIdentity {
                    uid: uid.parse().map_err(|_| invalid())?,
                    gid: gid.parse().map_err(|_| invalid())?,
                })
            })
            .transpose()
    };
    let native_roles = NativeRolesConfig {
        reader: identity("--native-reader")?,
        grant_administrator: identity("--native-grant-admin")?,
        maintenance: identity("--native-maintenance")?,
        backup_root: values.remove("--native-backup-root").map(PathBuf::from),
    };
    let config = ServiceConfig {
        native_sources,
        native_telemetry_bindings,
        native_roles,
        native_restore: values.remove("--native-restore"),
        native_content_config: values.remove("--native-content-config").map(PathBuf::from),
        data_root: PathBuf::from(values.remove("--data-root").ok_or_else(invalid)?),
        transport_dir: PathBuf::from(values.remove("--transport-dir").ok_or_else(invalid)?),
        receiver_id: values.remove("--receiver-id").ok_or_else(invalid)?,
        client_uid: values
            .remove("--client-uid")
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        client_gid: values
            .remove("--client-gid")
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        max_connections: values
            .remove("--max-connections")
            .map(|item| item.parse().map_err(|_| invalid()))
            .transpose()?,
        experimental_v2_scratch: match values.remove("--experimental-v2-scratch") {
            None => false,
            Some(value) if value == "enabled" => true,
            _ => return Err(invalid()),
        },
        experimental_real_clone_activation: match values
            .remove("--experimental-real-clone-activation")
        {
            None => false,
            Some(value) if value == "enabled" => true,
            _ => return Err(invalid()),
        },
    };
    run(&config)
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Agenturm storage service requires Linux peer credentials");
    std::process::exit(1);
}
