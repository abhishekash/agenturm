//! Authenticated loopback access to a dedicated broker reader.

mod api;
mod client;
mod http;

pub(crate) use client::RemoteStore;
pub use client::{RemoteHistoryOptions, execute_remote_cockpit};
pub(crate) use client::{open_workspace, parse as parse_attach};

use crate::WatchSignalRegistrations;
use crate::trace_receiver::ProducerToken;
use agenturm_storage_service::native_read::ReadFailure;
use agenturm_storage_service::{
    NativeClientError, NativeStore, ReaderHistoryReply, ReaderHistoryRequest, logical_read_identity,
};
use serde::{Deserialize, Serialize};
use std::io::BufReader;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const VERSION: u8 = 1;
const SERVER_USAGE: &str = "agenturm broker serve-history --broker-config <json> --listen <loopback-IP:port> --token-file <path> [--include-host]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReadServerOptions {
    listen: SocketAddr,
    token_file: PathBuf,
    include_host: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    history_identity: Option<String>,
    request: ReaderHistoryRequest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u8,
    history_identity: String,
    result: Result<ReaderHistoryReply, ReadFailure>,
}

pub(super) fn parse_server(args: &[String]) -> Result<ReadServerOptions, String> {
    let mut args = args.to_vec();
    let include_host = if let Some(index) = args.iter().position(|arg| arg == "--include-host") {
        if index % 2 != 0 {
            return Err(SERVER_USAGE.into());
        }
        args.remove(index);
        true
    } else {
        false
    };
    let (listen, token_file) = parse_address_and_token(&args, "--listen", SERVER_USAGE)?;
    let listen = loopback_address(listen, true)?;
    Ok(ReadServerOptions {
        listen,
        token_file,
        include_host,
    })
}

fn parse_address_and_token<'a>(
    args: &'a [String],
    address_flag: &str,
    usage: &str,
) -> Result<(&'a str, PathBuf), String> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(usage.into());
    }
    let (mut address, mut token_file) = (None, None);
    for [flag, value] in pairs {
        if value.is_empty() || value.starts_with("--") {
            return Err(usage.into());
        }
        if flag == address_flag && address.is_none() {
            address = Some(value.as_str());
        } else if flag == "--token-file" && token_file.is_none() {
            token_file = Some(PathBuf::from(value));
        } else {
            return Err(usage.into());
        }
    }
    Ok((address.ok_or(usage)?, token_file.ok_or(usage)?))
}

fn loopback_address(value: &str, allow_ephemeral: bool) -> Result<SocketAddr, String> {
    value
        .parse::<SocketAddr>()
        .ok()
        .filter(|address| address.ip().is_loopback() && (allow_ephemeral || address.port() != 0))
        .ok_or_else(|| "History read transport requires a literal loopback IP and port".to_owned())
}

pub(super) fn execute_server(
    options: &ReadServerOptions,
    store: NativeStore,
) -> Result<(), String> {
    let ReaderHistoryReply::Identity(identity) = store
        .reader_history(ReaderHistoryRequest::Identity)
        .map_err(|_| "The history read server requires the dedicated broker reader identity")?
    else {
        return Err("Invalid history identity response from broker".into());
    };
    if logical_read_identity(&identity) != Some(identity.as_str()) {
        return Err("Invalid history identity response from broker".into());
    }
    let store = store
        .with_workspace_identity(&identity)
        .map_err(|e| e.to_string())?;
    let binding = store.read_binding().ok_or("Missing history read binding")?;
    let token = ProducerToken::from_file(&options.token_file)?;
    let listener = TcpListener::bind(options.listen)
        .map_err(|error| format!("Bind history reader: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = WatchSignalRegistrations::new(Arc::clone(&stop))?;
    println!("Listening for history reads at http://{address}");
    serve(
        listener,
        &stop,
        &token,
        binding,
        options.include_host,
        |request| {
            store.reader_history(request).map_err(|error| match error {
                NativeClientError::Read(failure) => failure,
                _ => ReadFailure::Unavailable,
            })
        },
    )
}

fn serve(
    listener: TcpListener,
    stop: &AtomicBool,
    token: &ProducerToken,
    binding: &str,
    include_host: bool,
    read: impl Fn(ReaderHistoryRequest) -> Result<ReaderHistoryReply, ReadFailure>,
) -> Result<(), String> {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                handle(&stream, token, binding, include_host, &read)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => return Err(format!("Accept history read: {error}")),
        }
    }
    Ok(())
}

fn handle(
    stream: &TcpStream,
    token: &ProducerToken,
    binding: &str,
    include_host: bool,
    read: &impl Fn(ReaderHistoryRequest) -> Result<ReaderHistoryReply, ReadFailure>,
) {
    let read = |request| {
        if !include_host && matches!(&request, ReaderHistoryRequest::HostHistory { .. }) {
            return Err(ReadFailure::HostHistoryDisabled);
        }
        read(request)
    };
    let request_deadline = Instant::now() + http::IO_BUDGET;
    let mut reader = BufReader::new(stream);
    let response = (|| {
        let head = http::read_head(&mut reader, request_deadline, http::MAX_REQUEST)
            .map_err(|_| "400 Bad Request")?;
        let rest = if let Some(target) = head
            .start
            .strip_prefix("GET ")
            .and_then(|line| line.strip_suffix(" HTTP/1.1"))
        {
            Some(api::Request::parse(target)?)
        } else if head.start == format!("POST {} HTTP/1.1", http::PATH) {
            None
        } else {
            return Err("404 Not Found");
        };
        if head
            .host
            .as_deref()
            .is_none_or(|host| loopback_address(host, false).is_err())
        {
            return Err("400 Bad Request");
        }
        if !head
            .bearer
            .as_deref()
            .is_some_and(|header| token.authorizes(header))
        {
            return Err("401 Unauthorized");
        }
        if let Some(rest) = rest {
            if !rest.is_health() {
                match head.history_binding.as_deref() {
                    Some(expected) if logical_read_identity(expected).is_none() => {
                        return Err("400 Bad Request");
                    }
                    Some(expected) if expected == binding => {}
                    Some(_) => return Err("409 Conflict"),
                    None => return Err("428 Precondition Required"),
                }
            }
            let result = rest
                .execute(binding, &read)
                .map_err(|failure| match failure {
                    ReadFailure::Unauthorized | ReadFailure::HostHistoryDisabled => "403 Forbidden",
                    ReadFailure::InvalidRequest => "400 Bad Request",
                    ReadFailure::EvalInventoryTooLarge { .. } => "413 Content Too Large",
                    ReadFailure::HistoryReplaced
                    | ReadFailure::SnapshotReplaced
                    | ReadFailure::ExpiredSnapshot { .. } => "409 Conflict",
                    _ => "503 Service Unavailable",
                })?;
            let bytes = serde_json::to_vec(&result).map_err(|_| "503 Service Unavailable")?;
            return if bytes.len() <= http::MAX_REPLY {
                Ok(bytes)
            } else {
                Err("503 Service Unavailable")
            };
        }
        let bytes = http::read_body(&mut reader, request_deadline, head.length)
            .map_err(|_| "400 Bad Request")?;
        let frame: Request = serde_json::from_slice(&bytes).map_err(|_| "400 Bad Request")?;
        if frame.version != VERSION {
            return Err("400 Bad Request");
        }
        let result = match frame.history_identity.as_deref() {
            Some(expected) if logical_read_identity(expected).is_none() => {
                Err(ReadFailure::InvalidRequest)
            }
            Some(expected) if logical_read_identity(expected) != logical_read_identity(binding) => {
                Err(ReadFailure::HistoryReplaced)
            }
            Some(expected) if expected != binding => Err(ReadFailure::SnapshotReplaced),
            None if !matches!(frame.request, ReaderHistoryRequest::Identity) => {
                Err(ReadFailure::InvalidRequest)
            }
            _ => read(frame.request),
        };
        let reply = Reply {
            version: VERSION,
            history_identity: binding.into(),
            result,
        };
        let body = serde_json::to_vec(&reply).map_err(|_| "503 Service Unavailable")?;
        if body.len() > http::MAX_REPLY {
            return Err("503 Service Unavailable");
        }
        Ok(body)
    })();
    let (status, bytes) = match response {
        Ok(bytes) => ("200 OK", bytes),
        Err(status) => (
            status,
            serde_json::json!({"error":status}).to_string().into_bytes(),
        ),
    };
    let challenge = if status.starts_with("401") {
        "WWW-Authenticate: Bearer\r\n"
    } else {
        ""
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\n{challenge}Content-Length: {}\r\n\r\n",
        bytes.len()
    );
    let _ = http::write(stream, Instant::now() + http::IO_BUDGET, &head, &bytes);
}

#[cfg(test)]
mod http_api_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn host_history_flag_requires_an_explicit_standalone_opt_in() {
        let options = ["--listen", "127.0.0.1:4318", "--token-file", "token"].map(str::to_owned);
        assert!(!parse_server(&options).unwrap().include_host);
        for index in [0, 2, 4] {
            let mut opted_in = options.to_vec();
            opted_in.insert(index, "--include-host".into());
            assert!(parse_server(&opted_in).unwrap().include_host);
            opted_in.push("--include-host".into());
            assert!(parse_server(&opted_in).is_err());
        }
        let mut misplaced = options.to_vec();
        misplaced.insert(3, "--include-host".into());
        assert!(parse_server(&misplaced).is_err());
    }

    #[test]
    fn metadata_get_requires_bearer_and_current_binding_without_bodies_or_writes() {
        let root = std::env::temp_dir().join(format!("agenturm-get-api-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let token_path = root.join("token");
        std::fs::write(&token_path, "1".repeat(64)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let token = ProducerToken::from_file(&token_path).unwrap();
        let binding = "a".repeat(32);
        let calls = AtomicUsize::new(0);
        let round_trip = |target: &str, headers: &str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let (stream, _) = listener.accept().unwrap();
                    handle(&stream, &token, &binding, false, &|request| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        match request {
                            ReaderHistoryRequest::Identity => {
                                Ok(ReaderHistoryReply::Identity(binding.clone()))
                            }
                            ReaderHistoryRequest::Sessions { .. } => {
                                Ok(ReaderHistoryReply::Sessions(Default::default()))
                            }
                            _ => panic!("Unexpected metadata request"),
                        }
                    });
                });
                let mut client = TcpStream::connect(address).unwrap();
                client
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                write!(
                    client,
                    "GET {target} HTTP/1.1\r\nHost: {address}\r\n{headers}\r\n"
                )
                .unwrap();
                let mut response = String::new();
                client.read_to_string(&mut response).unwrap();
                response
            })
        };
        let authorization = format!("Authorization: Bearer {}\r\n", "1".repeat(64));
        assert!(round_trip("/api/health", "").starts_with("HTTP/1.1 401"));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let health = round_trip("/api/health", &authorization);
        assert!(health.starts_with("HTTP/1.1 200"));
        assert!(health.contains(&binding));
        assert!(round_trip("/api/sessions", &authorization).starts_with("HTTP/1.1 428"));
        let current = format!("{authorization}X-Agenturm-History-Binding: {binding}\r\n");
        assert!(round_trip("/api/sessions?limit=1", &current).starts_with("HTTP/1.1 200"));
        let read_count = calls.load(Ordering::Relaxed);
        assert!(round_trip("/api/host", "").starts_with("HTTP/1.1 401"));
        assert!(round_trip("/api/host", &authorization).starts_with("HTTP/1.1 428"));
        assert!(round_trip("/api/host", &current).starts_with("HTTP/1.1 403"));
        for extra in [
            "Content-Length: 1\r\n",
            "Origin: http://untrusted.example\r\n",
            "Transfer-Encoding: chunked\r\n",
            "X-Agenturm-History-Binding: duplicate\r\n",
        ] {
            assert!(
                round_trip("/api/sessions", &format!("{current}{extra}"))
                    .starts_with("HTTP/1.1 400")
            );
        }
        assert!(round_trip("/api/spans", &current).starts_with("HTTP/1.1 404"));
        assert!(
            round_trip("/api/sessions?source_id=otlp.a&source_id=otlp.b", &current)
                .starts_with("HTTP/1.1 400")
        );
        let other = "b".repeat(32);
        assert!(
            round_trip(
                "/api/sessions",
                &format!("{authorization}X-Agenturm-History-Binding: {other}\r\n")
            )
            .starts_with("HTTP/1.1 409")
        );
        assert_eq!(calls.load(Ordering::Relaxed), read_count);
        std::fs::remove_file(token_path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
