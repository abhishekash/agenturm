#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = start() {
        eprintln!("Agenturm storage service unavailable: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn start() -> std::io::Result<()> {
    use agenturm_storage_service::{ServiceConfig, run};
    use std::collections::HashMap;
    use std::io::{self, ErrorKind};
    use std::path::PathBuf;

    fn invalid() -> io::Error {
        io::Error::new(
            ErrorKind::InvalidInput,
            "expected --data-root PATH --transport-dir PATH --receiver-id ID --client-uid UID --client-gid GID [--max-connections N] [--experimental-v2-scratch enabled | --experimental-real-clone-activation enabled]",
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
        )
    }) {
        return Err(invalid());
    }
    let config = ServiceConfig {
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
