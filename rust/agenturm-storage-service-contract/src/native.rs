//! Versioned, one-exchange transport for standalone history. Operation DTOs
//! belong to the native service and do not share the legacy probe grammar.

use super::local::{DeadlineIo, IO_TIMEOUT, kernel_peer};
use super::{
    ContractError, PeerPolicy, Result, ServerPolicy, read_frame_bytes_bounded, valid_identity,
    valid_request_id, write_frame_bytes_bounded,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

pub const VERSION: u16 = 1;
pub const SOCKET_NAME: &str = "agenturm-native-storage-v1.sock";
/// 16 KiB per observation for a 2,048-row native store batch, including
/// bounded identity and fact JSON escaping. A batch is always one frame.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request<T> {
    pub version: u16,
    pub request_id: String,
    pub receiver_id: String,
    pub operation: T,
}

impl<T> Request<T> {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION
            || !valid_request_id(&self.request_id)
            || !valid_identity(&self.receiver_id)
        {
            return Err(ContractError::Invalid("invalid native request envelope"));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyIdentity {
    version: u16,
    request_id: String,
    receiver_id: String,
    request_sha256: [u8; 32],
}

impl ReplyIdentity {
    fn new<T>(request: &Request<T>, frame: &[u8]) -> Self {
        Self {
            version: request.version,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_sha256: Sha256::digest(frame).into(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reply<I, T> {
    identity: I,
    result: T,
}

pub fn send_request<T: Serialize>(
    stream: &mut UnixStream,
    request: &Request<T>,
    policy: ServerPolicy,
) -> Result<ReplyIdentity> {
    policy.authorize(kernel_peer(stream)?)?;
    request.validate()?;
    let frame = encode(request)?;
    send_frame(stream, &frame)?;
    Ok(ReplyIdentity::new(request, &frame))
}

/// Prove the peer's identity before allocating or consuming request bytes.
pub fn read_authorized_request<T: DeserializeOwned>(
    stream: &mut UnixStream,
    policy: PeerPolicy,
    expected_receiver_id: &str,
) -> Result<(T, ReplyIdentity)> {
    policy.authorize(kernel_peer(stream)?)?;
    let frame = read_frame(stream)?;
    let request: Request<T> = decode(&frame)?;
    request.validate()?;
    if request.receiver_id != expected_receiver_id {
        return Err(ContractError::Denied(
            "receiver identity differs from service",
        ));
    }
    let identity = ReplyIdentity::new(&request, &frame);
    Ok((request.operation, identity))
}

pub fn send_reply<T: Serialize>(
    stream: &mut UnixStream,
    identity: &ReplyIdentity,
    result: T,
) -> Result<()> {
    send_frame(stream, &encode(&Reply { identity, result })?)
}

pub fn read_reply<T: DeserializeOwned>(
    stream: &mut UnixStream,
    identity: &ReplyIdentity,
    policy: ServerPolicy,
) -> Result<T> {
    policy.authorize(kernel_peer(stream)?)?;
    let reply: Reply<ReplyIdentity, T> = decode(&read_frame(stream)?)?;
    if &reply.identity != identity {
        return Err(ContractError::Invalid(
            "native reply is for another request",
        ));
    }
    Ok(reply.result)
}

/// Connects without waiting on a full local listener backlog.
#[cfg(target_os = "linux")]
pub fn connect(path: &std::path::Path) -> std::io::Result<UnixStream> {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: zero is a valid initial value for sockaddr_un. The pathname
    // bytes below are bounded by its initialized sun_path buffer.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let name = path.as_os_str().as_bytes();
    if name.contains(&0) || name.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native service socket path is invalid",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(name) {
        *target = *byte as libc::c_char;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + name.len() + 1;
    // SAFETY: socket arguments use the Linux Unix stream protocol. The
    // successful descriptor is owned by this call and closed by OwnedFd.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: address and length describe the initialized pathname above.
    let status = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const address).cast(),
            length as libc::socklen_t,
        )
    };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    read_frame_bytes_bounded(&mut DeadlineIo::new(stream, IO_TIMEOUT), MAX_FRAME_BYTES)
}

fn send_frame(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    write_frame_bytes_bounded(
        &mut DeadlineIo::new(stream, IO_TIMEOUT),
        bytes,
        MAX_FRAME_BYTES,
    )?;
    stream.shutdown(Shutdown::Write)?;
    Ok(())
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|_| ContractError::Invalid("malformed or unsupported native JSON frame"))
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    struct BoundedFrame(Vec<u8>);

    impl std::io::Write for BoundedFrame {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_FRAME_BYTES - self.0.len() {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut frame = BoundedFrame(Vec::new());
    serde_json::to_writer(&mut frame, value)
        .map_err(|_| ContractError::Invalid("cannot encode bounded native response"))?;
    Ok(frame.0)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Operation {
        value: u64,
    }

    fn policies() -> (PeerPolicy, ServerPolicy) {
        // SAFETY: credential getters take no pointers.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        (
            PeerPolicy::synthetic_for_test(uid + 1, uid, gid).unwrap(),
            ServerPolicy::synthetic_for_test(uid, gid, uid + 1).unwrap(),
        )
    }

    fn request(value: u64) -> Request<Operation> {
        Request {
            version: VERSION,
            request_id: "0123456789abcdef0123456789abcdef".into(),
            receiver_id: "receiver:native".into(),
            operation: Operation { value },
        }
    }

    #[test]
    fn correlates_reply_with_exact_native_request_bytes() {
        let (mut client, mut service) = UnixStream::pair().unwrap();
        let (peer_policy, server_policy) = policies();
        let original = request(42);
        let id = send_request(&mut client, &original, server_policy).unwrap();
        let (operation, verified) =
            read_authorized_request::<Operation>(&mut service, peer_policy, "receiver:native")
                .unwrap();
        assert_eq!(operation, original.operation);
        assert_eq!(verified, id);
        send_reply(&mut service, &verified, 7u64).unwrap();
        assert_eq!(
            read_reply::<u64>(&mut client, &id, server_policy).unwrap(),
            7
        );

        let (mut client, mut service) = UnixStream::pair().unwrap();
        let changed = request(43);
        let wrong = ReplyIdentity::new(&changed, &encode(&changed).unwrap());
        send_reply(&mut service, &wrong, 7u64).unwrap();
        assert!(matches!(
            read_reply::<u64>(&mut client, &id, server_policy),
            Err(ContractError::Invalid(_))
        ));
    }

    #[test]
    fn checks_peers_before_io_and_rejects_invalid_frames() {
        let (mut client, mut service) = UnixStream::pair().unwrap();
        let (peer_policy, _) = policies();
        // SAFETY: credential getters take no pointers.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let denied = PeerPolicy::for_current_service(uid + 1, gid).unwrap();
        let too_large = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        client.write_all(&too_large).unwrap();
        assert!(matches!(
            read_authorized_request::<Operation>(&mut service, denied, "receiver:native"),
            Err(ContractError::Denied(_))
        ));
        assert!(matches!(
            read_authorized_request::<Operation>(&mut service, peer_policy, "receiver:native"),
            Err(ContractError::Invalid(_))
        ));

        let (mut client, mut service) = UnixStream::pair().unwrap();
        let wrong_server = ServerPolicy::for_current_client(uid + 1, gid).unwrap();
        assert!(matches!(
            send_request(&mut client, &request(42), wrong_server),
            Err(ContractError::Denied(_))
        ));
        service.set_nonblocking(true).unwrap();
        assert_eq!(
            service.read(&mut [0; 1]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );

        let (mut client, mut service) = UnixStream::pair().unwrap();
        let unrecognized = serde_json::json!({
            "version": VERSION,
            "request_id": "0123456789abcdef0123456789abcdef",
            "receiver_id": "receiver:native",
            "operation": {"value": 42, "unexpected": "deny"}
        });
        send_frame(&mut client, &encode(&unrecognized).unwrap()).unwrap();
        assert!(matches!(
            read_authorized_request::<Operation>(&mut service, peer_policy, "receiver:native"),
            Err(ContractError::Invalid(_))
        ));
    }

    #[test]
    fn client_reply_times_out_on_a_partial_native_frame() {
        let (mut client, mut service) = UnixStream::pair().unwrap();
        let (_, server_policy) = policies();
        let identity = send_request(&mut client, &request(42), server_policy).unwrap();
        service.write_all(&64_u32.to_be_bytes()).unwrap();
        service.write_all(b"x").unwrap();
        assert!(matches!(
            read_reply::<u64>(&mut client, &identity, server_policy),
            Err(ContractError::Io(error)) if error.kind() == std::io::ErrorKind::TimedOut
        ));
    }
}
