//! Host-owned transport for one credentialed status service.

use crate::connector::Outcome;
use agenturm_connector_api::OutboundDelivery;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// The outbound grant binds the exact transport and operator-managed references.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectorHostTransport {
    UnixStatusV1 {
        socket: PathBuf,
        peer_uid: u32,
        peer_gid: u32,
        credential_file: PathBuf,
    },
}

impl std::fmt::Debug for ConnectorHostTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("UnixStatusV1")
    }
}

impl ConnectorHostTransport {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let Self::UnixStatusV1 {
            socket,
            peer_uid,
            peer_gid,
            credential_file,
        } = self;
        if !valid_path(socket, 107)
            || !valid_path(credential_file, 4096)
            || socket == credential_file
            || *peer_uid == u32::MAX
            || *peer_gid == u32::MAX
        {
            return Err("connector_invalid_config");
        }
        Ok(())
    }
}

fn valid_path(path: &Path, max: usize) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && path.as_os_str().len() <= max
        && !path.as_os_str().as_encoded_bytes().contains(&0)
        && !path
            .components()
            .any(|component| component == Component::ParentDir)
}

#[derive(Clone, Copy)]
pub(crate) enum StatusAction {
    Deliver,
    Lookup,
}

pub(crate) fn exchange_status(
    transport: &ConnectorHostTransport,
    connector_id: &str,
    action: StatusAction,
    delivery: &OutboundDelivery,
    authorize: impl FnOnce() -> Result<(), &'static str>,
) -> Result<Outcome, &'static str> {
    #[cfg(target_os = "linux")]
    return linux::exchange(transport, connector_id, action, delivery, authorize);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (transport, connector_id, action, delivery, authorize);
        Err("connector_unsupported_platform")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use agenturm_connector_api::{DeliveryLookup, DeliveryReceipt};
    use sha2::{Digest, Sha256};
    use std::fs::{OpenOptions, symlink_metadata};
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    const VERSION: u16 = 1;
    const MAX_FRAME: usize = 4096;
    const MAX_TOKEN: usize = 256;
    const DEADLINE: Duration = Duration::from_secs(2);
    const DENIED: &str = "connector_host_permission_denied";
    const UNAVAILABLE: &str = "connector_host_unavailable";
    const PROTOCOL: &str = "connector_host_protocol_error";

    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum StatusOperation<'a> {
        Deliver { state: &'a str },
        Lookup,
    }

    #[derive(Serialize)]
    struct StatusRequest<'a> {
        version: u16,
        connector_id: &'a str,
        destination_id: &'a str,
        idempotency_key: &'a str,
        request_sha256: &'a str,
        operation: StatusOperation<'a>,
        credential: &'a str,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StatusReply {
        version: u16,
        destination_id: String,
        idempotency_key: String,
        request_sha256: String,
        status: ReplyStatus,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ReplyStatus {
        Accepted,
        ConfirmedMissing,
        Unknown,
    }

    pub(super) fn exchange(
        transport: &ConnectorHostTransport,
        connector_id: &str,
        action: StatusAction,
        delivery: &OutboundDelivery,
        authorize: impl FnOnce() -> Result<(), &'static str>,
    ) -> Result<Outcome, &'static str> {
        let ConnectorHostTransport::UnixStatusV1 {
            socket,
            peer_uid,
            peer_gid,
            credential_file,
        } = transport;
        let deadline = Instant::now() + DEADLINE;
        let endpoint = socket_identity(socket, *peer_uid)?;
        let stream = connect(socket)?;
        if !peer_matches(&stream, *peer_uid, *peer_gid)?
            || socket_identity(socket, *peer_uid)? != endpoint
        {
            return Err(DENIED);
        }
        let credential = credential(credential_file)?;
        let mut stream = DeadlineStream { stream, deadline };
        let operation = match action {
            StatusAction::Deliver => StatusOperation::Deliver {
                state: delivery.payload.as_str().ok_or(PROTOCOL)?,
            },
            StatusAction::Lookup => StatusOperation::Lookup,
        };
        let request = serde_json::to_string(&StatusRequest {
            version: VERSION,
            connector_id,
            destination_id: &delivery.destination_id,
            idempotency_key: &delivery.idempotency_key,
            request_sha256: &delivery.request_sha256,
            operation,
            credential: &credential,
        })
        .map_err(|_| PROTOCOL)?;
        authorize()?;
        agenturm_protocol::write_frame(&mut stream, &request, MAX_FRAME)
            .map_err(|_| UNAVAILABLE)?;
        drop(request);
        drop(credential);
        stream
            .stream
            .shutdown(Shutdown::Write)
            .map_err(|_| UNAVAILABLE)?;
        let reply = agenturm_protocol::read_frame(&mut stream, MAX_FRAME)
            .map_err(|_| UNAVAILABLE)?
            .ok_or(UNAVAILABLE)?;
        if stream.read(&mut [0]).map_err(|_| UNAVAILABLE)? != 0 {
            return Err(PROTOCOL);
        }
        let reply: StatusReply = serde_json::from_str(&reply).map_err(|_| PROTOCOL)?;
        if reply.version != VERSION
            || reply.destination_id != delivery.destination_id
            || reply.idempotency_key != delivery.idempotency_key
            || reply.request_sha256 != delivery.request_sha256
        {
            return Err(PROTOCOL);
        }
        result(action, reply, connector_id)
    }

    fn result(
        action: StatusAction,
        reply: StatusReply,
        connector_id: &str,
    ) -> Result<Outcome, &'static str> {
        let StatusReply {
            destination_id,
            idempotency_key,
            request_sha256,
            status,
            ..
        } = reply;
        // Validate the round-trip identifiers above, and derive the public
        // receipt locally. The endpoint supplies no arbitrary durable ID.
        let receipt = || {
            let mut digest = Sha256::new();
            for field in [
                "agenturm.connector-unix-status/1",
                connector_id,
                &destination_id,
                &idempotency_key,
                &request_sha256,
            ] {
                digest.update(field);
                digest.update([0]);
            }
            format!("host-status-{:x}", digest.finalize())
        };
        let outcome = match (action, status) {
            (StatusAction::Deliver, ReplyStatus::Accepted) => Outcome::Delivered {
                receipt: DeliveryReceipt {
                    receipt_id: receipt(),
                    destination_id,
                    idempotency_key,
                    request_sha256,
                },
            },
            (StatusAction::Lookup, ReplyStatus::Accepted) => Outcome::Lookup {
                result: DeliveryLookup::Accepted {
                    receipt_id: receipt(),
                    destination_id,
                    idempotency_key,
                    request_sha256,
                },
            },
            (StatusAction::Lookup, ReplyStatus::ConfirmedMissing) => Outcome::Lookup {
                result: DeliveryLookup::ConfirmedMissing {
                    destination_id,
                    idempotency_key,
                    request_sha256,
                },
            },
            (StatusAction::Lookup, ReplyStatus::Unknown) => Outcome::Lookup {
                result: DeliveryLookup::Unknown {
                    destination_id,
                    idempotency_key,
                    request_sha256,
                },
            },
            _ => return Err(PROTOCOL),
        };
        Ok(outcome)
    }

    fn credential(path: &Path) -> Result<String, &'static str> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| DENIED)?;
        let metadata = file.metadata().map_err(|_| DENIED)?;
        // SAFETY: the credential getter takes no pointers.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.uid() != owner
            || metadata.nlink() != 1
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > (MAX_TOKEN + 1) as u64
        {
            return Err(DENIED);
        }
        let mut token = String::new();
        file.take((MAX_TOKEN + 2) as u64)
            .read_to_string(&mut token)
            .map_err(|_| DENIED)?;
        if token.ends_with('\n') {
            token.pop();
        }
        if !(32..=MAX_TOKEN).contains(&token.len())
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(DENIED);
        }
        Ok(token)
    }

    fn socket_identity(path: &Path, owner: u32) -> Result<(u64, u64), &'static str> {
        let metadata = symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
        if !metadata.file_type().is_socket() || metadata.uid() != owner {
            return Err(DENIED);
        }
        Ok((metadata.dev(), metadata.ino()))
    }

    fn peer_matches(stream: &UnixStream, uid: u32, gid: u32) -> Result<bool, &'static str> {
        // SAFETY: ucred's integer fields allow zero initialization. The kernel
        // writes only to this struct and the provided length.
        let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = size_of::<libc::ucred>() as libc::socklen_t;
        let status = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut peer).cast(),
                &mut len,
            )
        };
        if status < 0 || len as usize != size_of::<libc::ucred>() {
            return Err(DENIED);
        }
        Ok(peer.pid > 0 && peer.uid == uid && peer.gid == gid)
    }

    fn connect(path: &Path) -> Result<UnixStream, &'static str> {
        // SAFETY: the Linux socket address is valid zero-filled. The pathname
        // is copied only into its bounded array.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        let name = path.as_os_str().as_bytes();
        if name.len() >= address.sun_path.len() {
            return Err(DENIED);
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (target, byte) in address.sun_path.iter_mut().zip(name) {
            *target = *byte as libc::c_char;
        }
        let len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + name.len() + 1;
        // SAFETY: these are the Linux Unix-stream socket arguments. On
        // success we take exclusive ownership of the new descriptor.
        let fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(UNAVAILABLE);
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: address and length describe the initialized pathname above.
        let status = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&raw const address).cast(),
                len as libc::socklen_t,
            )
        };
        if status != 0 {
            return Err(UNAVAILABLE);
        }
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(false).map_err(|_| UNAVAILABLE)?;
        Ok(stream)
    }

    struct DeadlineStream {
        stream: UnixStream,
        deadline: Instant,
    }

    impl DeadlineStream {
        fn remaining(&self) -> io::Result<Duration> {
            self.deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or(io::ErrorKind::TimedOut.into())
        }
    }

    impl Read for DeadlineStream {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.stream.set_read_timeout(Some(self.remaining()?))?;
            self.stream.read(bytes)
        }
    }

    impl Write for DeadlineStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.stream.set_write_timeout(Some(self.remaining()?))?;
            self.stream.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.stream.flush()
        }
    }
}
