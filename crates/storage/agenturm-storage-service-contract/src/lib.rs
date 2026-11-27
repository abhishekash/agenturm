//! Local storage-service transport contracts.
//!
//! This crate has no listener, database handle, or production caller. V1 is a
//! one-request-per-connection protocol probe; v2 framing is used only by an
//! explicitly enabled disposable scratch service. Native standalone history
//! uses a separate endpoint and request envelope.

#![forbid(unsafe_op_in_unsafe_fn)]

/// Typed v2 lifecycle admission contract. Only experimental scratch dispatch
/// accepts this version; the ordinary v1 probe remains version-isolated.
pub mod v2;

#[cfg(unix)]
pub mod native;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read, Write};

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum ContractError {
    Io(io::Error),
    Invalid(&'static str),
    Denied(&'static str),
}

impl fmt::Display for ContractError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(out, "local service I/O: {error}"),
            Self::Invalid(reason) => write!(out, "invalid local service frame: {reason}"),
            Self::Denied(reason) => write!(out, "local service peer denied: {reason}"),
        }
    }
}

impl std::error::Error for ContractError {}

impl From<io::Error> for ContractError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T> = std::result::Result<T, ContractError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub request_id: String,
    pub receiver_id: String,
    pub operation: Operation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Probe { topic: ProbeTopic },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeTopic {
    Protocol,
    ReceiverIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u16,
    pub request_id: String,
    pub receiver_id: String,
    pub result: ProbeResult,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeResult {
    ProtocolReady,
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn valid_request_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Request {
    pub fn probe(request_id: String, receiver_id: String, topic: ProbeTopic) -> Result<Self> {
        let request = Self {
            version: VERSION,
            request_id,
            receiver_id,
            operation: Operation::Probe { topic },
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(ContractError::Invalid("unsupported protocol version"));
        }
        if !valid_request_id(&self.request_id) {
            return Err(ContractError::Invalid("invalid request ID"));
        }
        if !valid_identity(&self.receiver_id) {
            return Err(ContractError::Invalid("invalid receiver ID"));
        }
        Ok(())
    }

    pub fn replay_identity(&self) -> Result<ReplayIdentity> {
        self.validate()?;
        // Canonical serialization gives equivalent JSON one digest.
        let canonical = serde_json::to_vec(self).expect("fixed v1 request is serializable");
        Ok(ReplayIdentity {
            request_id: self.request_id.clone(),
            receiver_id: self.receiver_id.clone(),
            digest: Sha256::digest(canonical).into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayIdentity {
    request_id: String,
    receiver_id: String,
    digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayRelation {
    DifferentRequest,
    Exact,
    Conflict,
}

impl ReplayIdentity {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn receiver_id(&self) -> &str {
        &self.receiver_id
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// Pure comparison only. A future writer must persist receipts atomically.
    pub fn compare(&self, next: &Self) -> ReplayRelation {
        if self.request_id != next.request_id {
            ReplayRelation::DifferentRequest
        } else if self.receiver_id == next.receiver_id && self.digest == next.digest {
            ReplayRelation::Exact
        } else {
            ReplayRelation::Conflict
        }
    }
}

fn read_frame_bytes(reader: &mut impl Read) -> Result<Vec<u8>> {
    read_frame_bytes_bounded(reader, MAX_FRAME_BYTES)
}

fn read_frame_bytes_bounded(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let data = read_sized_frame(reader, limit)?;
    require_frame_eof(reader)?;
    Ok(data)
}

fn read_sized_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > limit {
        return Err(ContractError::Invalid("frame length exceeds contract"));
    }
    let mut data = vec![0_u8; length];
    reader.read_exact(&mut data)?;
    Ok(data)
}

fn require_frame_eof(reader: &mut impl Read) -> Result<()> {
    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing)? != 0 {
        return Err(ContractError::Invalid("trailing frame data"));
    }
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<T> {
    serde_json::from_slice(&read_frame_bytes(reader)?)
        .map_err(|_| ContractError::Invalid("malformed or unsupported JSON frame"))
}

fn write_frame_bytes(writer: &mut impl Write, data: &[u8]) -> Result<()> {
    write_frame_bytes_bounded(writer, data, MAX_FRAME_BYTES)
}

fn write_frame_bytes_bounded(writer: &mut impl Write, data: &[u8], limit: usize) -> Result<()> {
    if data.is_empty() || data.len() > limit {
        return Err(ContractError::Invalid("frame length exceeds contract"));
    }
    writer.write_all(&(data.len() as u32).to_be_bytes())?;
    writer.write_all(data)?;
    writer.flush()?;
    Ok(())
}

fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)
        .map_err(|_| ContractError::Invalid("response serialization failed"))?;
    write_frame_bytes(writer, &data)
}

#[cfg(unix)]
mod local {
    use super::{
        ContractError, ProbeResult, Reply, Request, Result, VERSION, read_frame, read_frame_bytes,
        write_frame, write_frame_bytes,
    };
    use std::io::{self, ErrorKind, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(2);

    pub(super) struct DeadlineIo<'a> {
        stream: &'a mut UnixStream,
        deadline: Instant,
    }

    impl<'a> DeadlineIo<'a> {
        pub(super) fn new(stream: &'a mut UnixStream, timeout: Duration) -> Self {
            Self {
                stream,
                deadline: Instant::now() + timeout,
            }
        }

        fn remaining(&self) -> io::Result<Duration> {
            self.deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| io::Error::new(ErrorKind::TimedOut, "frame deadline exceeded"))
        }

        fn timeout_error(error: io::Error) -> io::Error {
            // Linux returns WouldBlock when a blocking socket's SO_RCVTIMEO or
            // SO_SNDTIMEO expires. Both spellings mean this frame's remaining
            // deadline elapsed; a nonblocking peer also fails closed.
            if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                io::Error::new(ErrorKind::TimedOut, "frame deadline exceeded")
            } else {
                error
            }
        }
    }

    impl Read for DeadlineIo<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.stream.set_read_timeout(Some(self.remaining()?))?;
            let count = self.stream.read(buffer).map_err(Self::timeout_error)?;
            self.remaining()?;
            Ok(count)
        }
    }

    impl Write for DeadlineIo<'_> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.stream.set_write_timeout(Some(self.remaining()?))?;
            let count = self.stream.write(buffer).map_err(Self::timeout_error)?;
            self.remaining()?;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.remaining()?;
            self.stream.flush()
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct PeerCredentials {
        pid: u32,
        uid: u32,
        gid: u32,
    }

    impl PeerCredentials {
        pub fn pid(self) -> u32 {
            self.pid
        }
        pub fn uid(self) -> u32 {
            self.uid
        }
        pub fn gid(self) -> u32 {
            self.gid
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct PeerPolicy {
        service_uid: u32,
        service_pid: u32,
        client_uid: u32,
        client_gid: u32,
        verify_service_uid: bool,
    }

    impl PeerPolicy {
        /// Bind the policy to this process's effective service UID after any
        /// privilege drop. The client must be a different, non-root account.
        pub fn for_current_service(client_uid: u32, client_gid: u32) -> Result<Self> {
            // SAFETY: geteuid takes no pointers or caller-controlled input.
            let service_uid = unsafe { libc::geteuid() };
            Self::from_ids(service_uid, client_uid, client_gid, true)
        }

        fn from_ids(
            service_uid: u32,
            client_uid: u32,
            client_gid: u32,
            verify_service_uid: bool,
        ) -> Result<Self> {
            if service_uid == 0 || client_uid == 0 || client_gid == 0 || service_uid == client_uid {
                return Err(ContractError::Denied(
                    "distinct non-root service and client credentials required",
                ));
            }
            Ok(Self {
                service_uid,
                service_pid: std::process::id(),
                client_uid,
                client_gid,
                verify_service_uid,
            })
        }

        #[cfg(test)]
        pub(super) fn synthetic_for_test(
            service_uid: u32,
            client_uid: u32,
            client_gid: u32,
        ) -> Result<Self> {
            Self::from_ids(service_uid, client_uid, client_gid, false)
        }

        pub(super) fn authorize(self, peer: PeerCredentials) -> Result<PeerCredentials> {
            // SAFETY: geteuid takes no pointers or caller-controlled input.
            let actual_service_uid = unsafe { libc::geteuid() };
            if self.verify_service_uid
                && (actual_service_uid != self.service_uid
                    || std::process::id() != self.service_pid)
            {
                return Err(ContractError::Denied(
                    "service identity changed after policy construction",
                ));
            }
            if peer.pid == 0
                || peer.uid != self.client_uid
                || peer.gid != self.client_gid
                || peer.uid == self.service_uid
            {
                return Err(ContractError::Denied(
                    "peer credentials do not match client grant",
                ));
            }
            Ok(peer)
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct ServerPolicy {
        service_uid: u32,
        service_gid: u32,
        client_uid: u32,
        client_pid: u32,
        verify_client_uid: bool,
    }

    impl ServerPolicy {
        /// Bind the client process after any privilege drop and pin the
        /// expected distinct, non-root service account.
        pub fn for_current_client(service_uid: u32, service_gid: u32) -> Result<Self> {
            // SAFETY: geteuid takes no pointers or caller-controlled input.
            let client_uid = unsafe { libc::geteuid() };
            Self::from_ids(service_uid, service_gid, client_uid, true)
        }

        fn from_ids(
            service_uid: u32,
            service_gid: u32,
            client_uid: u32,
            verify_client_uid: bool,
        ) -> Result<Self> {
            if service_uid == 0 || service_gid == 0 || client_uid == 0 || service_uid == client_uid
            {
                return Err(ContractError::Denied(
                    "distinct non-root service and client credentials required",
                ));
            }
            Ok(Self {
                service_uid,
                service_gid,
                client_uid,
                client_pid: std::process::id(),
                verify_client_uid,
            })
        }

        #[cfg(test)]
        pub(super) fn synthetic_for_test(
            service_uid: u32,
            service_gid: u32,
            client_uid: u32,
        ) -> Result<Self> {
            Self::from_ids(service_uid, service_gid, client_uid, false)
        }

        pub(super) fn authorize(self, peer: PeerCredentials) -> Result<()> {
            // SAFETY: geteuid takes no pointers or caller-controlled input.
            let actual_client_uid = unsafe { libc::geteuid() };
            if self.verify_client_uid
                && (actual_client_uid != self.client_uid || std::process::id() != self.client_pid)
            {
                return Err(ContractError::Denied(
                    "client identity changed after policy construction",
                ));
            }
            if peer.pid == 0
                || peer.uid != self.service_uid
                || peer.gid != self.service_gid
                || peer.uid == self.client_uid
            {
                return Err(ContractError::Denied(
                    "peer credentials do not match service grant",
                ));
            }
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn kernel_peer(stream: &UnixStream) -> Result<PeerCredentials> {
        use std::mem::{MaybeUninit, size_of};
        use std::os::fd::AsRawFd;

        let mut raw = MaybeUninit::<libc::ucred>::uninit();
        let mut size = size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: raw points to writable storage of the declared size, size
        // points to a valid socklen_t, and the stream owns a live socket FD.
        let status = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                raw.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if size as usize != size_of::<libc::ucred>() {
            return Err(ContractError::Denied("incomplete kernel peer credentials"));
        }
        // SAFETY: getsockopt returned the full ucred size above.
        let raw = unsafe { raw.assume_init() };
        if raw.pid <= 0 {
            return Err(ContractError::Denied("invalid kernel peer PID"));
        }
        Ok(PeerCredentials {
            pid: raw.pid as u32,
            uid: raw.uid,
            gid: raw.gid,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn kernel_peer(_stream: &UnixStream) -> Result<PeerCredentials> {
        Err(ContractError::Denied(
            "kernel peer credentials unavailable on this platform",
        ))
    }

    pub struct AuthorizedRequest {
        request: Request,
        peer: PeerCredentials,
    }

    /// A peer-authorized v2 frame. The original bytes are passed only to the
    /// explicit scratch admission bridge after its typed contract is checked.
    pub struct AuthorizedV2Request {
        request: crate::v2::Request,
        frame: Vec<u8>,
    }

    impl AuthorizedV2Request {
        pub fn request(&self) -> &crate::v2::Request {
            &self.request
        }

        pub fn frame(&self) -> &[u8] {
            &self.frame
        }
    }

    pub fn read_authorized_v2_request(
        stream: &mut UnixStream,
        policy: PeerPolicy,
        expected_receiver_id: &str,
    ) -> Result<AuthorizedV2Request> {
        policy.authorize(kernel_peer(stream)?)?;
        let frame = read_frame_bytes(&mut DeadlineIo::new(stream, IO_TIMEOUT))?;
        let request = crate::v2::Request::from_json(&frame)?;
        if request.receiver_id != expected_receiver_id {
            return Err(ContractError::Denied(
                "receiver identity differs from service",
            ));
        }
        Ok(AuthorizedV2Request { request, frame })
    }

    pub fn send_v2_reply(
        stream: &mut UnixStream,
        request: &AuthorizedV2Request,
        frame: &[u8],
    ) -> Result<()> {
        crate::v2::Reply::from_json(frame, &request.request)?;
        write_frame_bytes(&mut DeadlineIo::new(stream, IO_TIMEOUT), frame)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    }

    /// Send one canonical v2 request only to the exact service principal.
    /// Peer validation must precede all request bytes because lifecycle
    /// evidence must not be disclosed to an unrelated socket listener.
    pub fn send_v2_request(
        stream: &mut UnixStream,
        request: &crate::v2::Request,
        server_policy: ServerPolicy,
    ) -> Result<()> {
        let frame = request.to_json()?;
        server_policy.authorize(kernel_peer(stream)?)?;
        write_frame_bytes(&mut DeadlineIo::new(stream, IO_TIMEOUT), &frame)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    }

    fn read_v2_reply_with_timeout(
        stream: &mut UnixStream,
        request: &crate::v2::Request,
        server_policy: ServerPolicy,
        timeout: Duration,
    ) -> Result<crate::v2::Reply> {
        request.validate()?;
        // Verify the kernel peer before consuming any server-controlled bytes.
        server_policy.authorize(kernel_peer(stream)?)?;
        let frame = read_frame_bytes(&mut DeadlineIo::new(stream, timeout))?;
        crate::v2::Reply::from_json(&frame, request)
    }

    /// Accept only a one-frame reply from the exact service principal and
    /// bind its digest, IDs, cursor and batch to the original v2 request.
    pub fn read_v2_reply(
        stream: &mut UnixStream,
        request: &crate::v2::Request,
        server_policy: ServerPolicy,
    ) -> Result<crate::v2::Reply> {
        read_v2_reply_with_timeout(stream, request, server_policy, IO_TIMEOUT)
    }

    impl AuthorizedRequest {
        pub fn request(&self) -> &Request {
            &self.request
        }
        pub fn peer(&self) -> PeerCredentials {
            self.peer
        }
    }

    /// Inspect kernel credentials before parsing any client-controlled bytes.
    /// One request per connection; the client must half-close its write side.
    pub fn read_authorized_request(
        stream: &mut UnixStream,
        policy: PeerPolicy,
        expected_receiver_id: &str,
    ) -> Result<AuthorizedRequest> {
        let peer = policy.authorize(kernel_peer(stream)?)?;
        let request: Request = read_frame(&mut DeadlineIo::new(stream, IO_TIMEOUT))?;
        request.validate()?;
        if request.receiver_id != expected_receiver_id {
            return Err(ContractError::Denied(
                "receiver identity differs from service",
            ));
        }
        Ok(AuthorizedRequest { request, peer })
    }

    pub fn send_request(stream: &mut UnixStream, request: &Request) -> Result<()> {
        request.validate()?;
        write_frame(&mut DeadlineIo::new(stream, IO_TIMEOUT), request)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    }

    /// This reply means only that the protocol/peer gate succeeded. It says
    /// nothing about a storage writer, database readiness, or capture policy.
    pub fn send_probe_reply(stream: &mut UnixStream, request: &AuthorizedRequest) -> Result<()> {
        let reply = Reply {
            version: VERSION,
            request_id: request.request.request_id.clone(),
            receiver_id: request.request.receiver_id.clone(),
            result: ProbeResult::ProtocolReady,
        };
        write_frame(&mut DeadlineIo::new(stream, IO_TIMEOUT), &reply)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    }

    pub fn read_probe_reply(
        stream: &mut UnixStream,
        request: &Request,
        server_policy: ServerPolicy,
    ) -> Result<Reply> {
        request.validate()?;
        server_policy.authorize(kernel_peer(stream)?)?;
        let reply: Reply = read_frame(&mut DeadlineIo::new(stream, IO_TIMEOUT))?;
        if reply.version != VERSION
            || reply.request_id != request.request_id
            || reply.receiver_id != request.receiver_id
            || reply.result != ProbeResult::ProtocolReady
        {
            return Err(ContractError::Invalid("reply identity or version mismatch"));
        }
        Ok(reply)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{Operation, ProbeTopic, ReplayRelation};
        use serde::Deserialize;
        use serde_json::{Value, json};
        use std::fmt::Debug;
        use std::io::Write;
        use std::sync::mpsc;
        use std::thread;

        fn sample() -> Request {
            Request::probe(
                "0123456789abcdef0123456789abcdef".into(),
                "receiver:one".into(),
                ProbeTopic::Protocol,
            )
            .unwrap()
        }

        fn sample_v2() -> crate::v2::Request {
            let fixture: Value = serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../test-support/fixtures/rust_storage_mutation_v2.json"
            )))
            .unwrap();
            crate::v2::Request::from_json(&serde_json::to_vec(&fixture["request"]).unwrap())
                .unwrap()
        }

        fn v2_reply(request: &crate::v2::Request) -> crate::v2::Reply {
            let crate::v2::Operation::AdmitLifecycle {
                cursor,
                batch_digest,
                ..
            } = &request.operation
            else {
                panic!("fixture is lifecycle admission")
            };
            crate::v2::Reply {
                version: crate::v2::VERSION,
                request_id: request.request_id.clone(),
                receiver_id: request.receiver_id.clone(),
                request_digest: request.replay_identity().unwrap().digest,
                result: crate::v2::OperationResult::LifecycleAdmission(
                    crate::v2::AdmissionResult {
                        disposition: crate::v2::Disposition::Committed,
                        cursor: cursor.clone(),
                        batch_digest: batch_digest.clone(),
                        dirty_seq: 1,
                    },
                ),
            }
        }

        #[cfg(target_os = "linux")]
        fn send_raw(client: &mut UnixStream, raw: &str) {
            client.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
            client.write_all(raw.as_bytes()).unwrap();
            client.shutdown(std::net::Shutdown::Write).unwrap();
        }

        #[cfg(target_os = "linux")]
        fn policy() -> PeerPolicy {
            // Tests run as a regular user; the service identity remains
            // distinct and is never actually installed or started here.
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            PeerPolicy::synthetic_for_test(uid + 1, uid, gid).unwrap()
        }

        #[cfg(target_os = "linux")]
        fn server_policy() -> ServerPolicy {
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            ServerPolicy::synthetic_for_test(uid, gid, uid + 1).unwrap()
        }

        fn assert_total_deadline_for_drip<T: for<'de> Deserialize<'de> + Debug>() {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            // The first payload byte is already available. Each subsequent
            // byte arrives before the per-read timeout but the complete frame
            // will not arrive before the single total deadline.
            sender.write_all(&16_u32.to_be_bytes()).unwrap();
            sender.write_all(b"x").unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            let producer = thread::spawn(move || {
                ready_tx.send(()).unwrap();
                for _ in 0..8 {
                    thread::sleep(Duration::from_millis(50));
                    if sender.write_all(b"x").is_err() {
                        break;
                    }
                }
            });
            ready_rx.recv().unwrap();
            let mut bounded = DeadlineIo::new(&mut receiver, Duration::from_millis(180));
            assert!(matches!(
                read_frame::<T>(&mut bounded),
                Err(ContractError::Io(ref error)) if error.kind() == ErrorKind::TimedOut
            ));
            producer.join().unwrap();
        }

        #[test]
        fn request_and_response_have_one_total_frame_deadline() {
            assert_total_deadline_for_drip::<Request>();
            assert_total_deadline_for_drip::<Reply>();
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn kernel_peer_and_exact_round_trip() {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let request = sample();
            send_request(&mut client, &request).unwrap();
            let admitted = read_authorized_request(&mut server, policy(), "receiver:one").unwrap();
            assert_eq!(admitted.request(), &request);
            assert_eq!(admitted.peer().uid(), unsafe { libc::geteuid() });
            assert_eq!(admitted.peer().gid(), unsafe { libc::getegid() });
            assert!(admitted.peer().pid() > 0);
            send_probe_reply(&mut server, &admitted).unwrap();
            assert_eq!(
                read_probe_reply(&mut client, &request, server_policy())
                    .unwrap()
                    .result,
                ProbeResult::ProtocolReady
            );
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn denied_peer_is_checked_before_untrusted_bytes() {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            client.write_all(&[0, 0, 0, 1, b'!']).unwrap();
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            let denied = PeerPolicy::synthetic_for_test(uid + 1, uid + 2, gid).unwrap();
            assert!(matches!(
                read_authorized_request(&mut server, denied, "receiver:one"),
                Err(ContractError::Denied(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn wrong_receiver_is_denied() {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            send_request(&mut client, &sample()).unwrap();
            assert!(matches!(
                read_authorized_request(&mut server, policy(), "receiver:two"),
                Err(ContractError::Denied(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn malformed_oversized_and_trailing_frames_fail() {
            for bytes in [
                vec![0, 0, 0, 1, b'!'],
                ((crate::MAX_FRAME_BYTES as u32) + 1).to_be_bytes().to_vec(),
                vec![0, 0, 0, 2, b'{', b'}', b'!'],
            ] {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                client.write_all(&bytes).unwrap();
                client.shutdown(std::net::Shutdown::Write).unwrap();
                assert!(matches!(
                    read_authorized_request(&mut server, policy(), "receiver:one"),
                    Err(ContractError::Invalid(_))
                ));
            }
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn incomplete_frames_fail_without_reusing_partial_data() {
            for bytes in [vec![0, 0], vec![0, 0, 0, 4, b'{']] {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                client.write_all(&bytes).unwrap();
                client.shutdown(std::net::Shutdown::Write).unwrap();
                assert!(matches!(
                    read_authorized_request(&mut server, policy(), "receiver:one"),
                    Err(ContractError::Io(_))
                ));
            }
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn unknown_mutation_and_duplicate_keys_fail_closed() {
            let request = sample();
            let canonical = serde_json::to_string(&request).unwrap();
            for raw in [
                canonical.replace("\"probe\"", "\"commit\""),
                canonical.replace("\"version\":1", "\"version\":2"),
                canonical.replace("0123456789abcdef0123456789abcdef", "BAD"),
                canonical.replace("\"version\":1", "\"version\":1,\"version\":1"),
                canonical.replace("\"op\":\"probe\"", "\"op\":\"probe\",\"path\":\"/tmp/x\""),
            ] {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                send_raw(&mut client, &raw);
                assert!(matches!(
                    read_authorized_request(&mut server, policy(), "receiver:one"),
                    Err(ContractError::Invalid(_))
                ));
            }
        }

        #[test]
        fn replay_identity_is_exact_and_scoped() {
            let request = sample();
            let same = request.clone();
            assert_eq!(
                request
                    .replay_identity()
                    .unwrap()
                    .compare(&same.replay_identity().unwrap()),
                ReplayRelation::Exact
            );
            let mut changed = request.clone();
            changed.operation = Operation::Probe {
                topic: ProbeTopic::ReceiverIdentity,
            };
            assert_eq!(
                request
                    .replay_identity()
                    .unwrap()
                    .compare(&changed.replay_identity().unwrap()),
                ReplayRelation::Conflict
            );
            changed = request.clone();
            changed.receiver_id = "receiver:two".into();
            assert_eq!(
                request
                    .replay_identity()
                    .unwrap()
                    .compare(&changed.replay_identity().unwrap()),
                ReplayRelation::Conflict
            );
            changed = request.clone();
            changed.request_id = "ffffffffffffffffffffffffffffffff".into();
            assert_eq!(
                request
                    .replay_identity()
                    .unwrap()
                    .compare(&changed.replay_identity().unwrap()),
                ReplayRelation::DifferentRequest
            );
        }

        #[test]
        fn invalid_ids_and_unsafe_policy_fail() {
            assert!(
                Request::probe("ABC".into(), "receiver:one".into(), ProbeTopic::Protocol).is_err()
            );
            assert!(
                Request::probe("a".repeat(32), "../other".into(), ProbeTopic::Protocol).is_err()
            );
            assert!(PeerPolicy::synthetic_for_test(1000, 1000, 1000).is_err());
            assert!(PeerPolicy::synthetic_for_test(0, 1000, 1000).is_err());
            assert!(PeerPolicy::synthetic_for_test(1001, 0, 1000).is_err());
            assert!(PeerPolicy::synthetic_for_test(1001, 1000, 0).is_err());
            assert!(ServerPolicy::synthetic_for_test(1001, 0, 1000).is_err());
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            assert!(PeerPolicy::for_current_service(uid, gid).is_err());
            assert!(ServerPolicy::for_current_client(uid, gid).is_err());
            let mut policy = PeerPolicy::for_current_service(uid + 1, gid).unwrap();
            policy.service_pid = 0;
            assert!(matches!(
                policy.authorize(PeerCredentials {
                    pid: 1,
                    uid: uid + 1,
                    gid
                }),
                Err(ContractError::Denied(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn wrong_reply_id_fails() {
            let request = sample();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let wrong = Reply {
                version: VERSION,
                request_id: "f".repeat(32),
                receiver_id: request.receiver_id.clone(),
                result: ProbeResult::ProtocolReady,
            };
            write_frame(&mut server, &wrong).unwrap();
            server.shutdown(std::net::Shutdown::Write).unwrap();
            assert!(matches!(
                read_probe_reply(&mut client, &request, server_policy()),
                Err(ContractError::Invalid(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn wrong_server_peer_is_denied_before_reply_parse() {
            let request = sample();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            server.write_all(&[0, 0, 0, 1, b'!']).unwrap();
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            let denied = ServerPolicy::synthetic_for_test(uid + 2, gid, uid + 1).unwrap();
            assert!(matches!(
                read_probe_reply(&mut client, &request, denied),
                Err(ContractError::Denied(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn exact_v2_request_and_reply_round_trip() {
            let request = sample_v2();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            send_v2_request(&mut client, &request, server_policy()).unwrap();
            let admitted =
                read_authorized_v2_request(&mut server, policy(), &request.receiver_id).unwrap();
            assert_eq!(admitted.request(), &request);
            assert_eq!(admitted.frame(), request.to_json().unwrap());
            let frame = v2_reply(&request).to_json(&request).unwrap();
            send_v2_reply(&mut server, &admitted, &frame).unwrap();
            assert_eq!(
                read_v2_reply(&mut client, &request, server_policy()).unwrap(),
                v2_reply(&request)
            );
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn v2_reply_rejects_wrong_service_before_reading_frame() {
            let request = sample_v2();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            server.write_all(&[0, 0, 0, 1, b'!']).unwrap();
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            let wrong = ServerPolicy::synthetic_for_test(uid + 2, gid, uid + 1).unwrap();
            assert!(matches!(
                read_v2_reply(&mut client, &request, wrong),
                Err(ContractError::Denied(_))
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn v2_request_rejects_wrong_service_without_disclosing_bytes() {
            let request = sample_v2();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            let wrong = ServerPolicy::synthetic_for_test(uid + 2, gid, uid + 1).unwrap();
            assert!(matches!(
                send_v2_request(&mut client, &request, wrong),
                Err(ContractError::Denied(_))
            ));
            server
                .set_read_timeout(Some(Duration::from_millis(20)))
                .unwrap();
            let mut byte = [0_u8; 1];
            assert!(matches!(
                server.read(&mut byte),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
            ));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn v2_reply_rejects_malformed_truncated_oversized_duplicate_and_trailing_frames() {
            let request = sample_v2();
            let valid = v2_reply(&request).to_json(&request).unwrap();
            let raw = String::from_utf8(valid.clone()).unwrap();
            let duplicate = raw.replacen("\"version\":2", "\"version\":2,\"version\":2", 1);
            let duplicate_len = duplicate.len();
            let cases = [
                (vec![0, 0, 0, 1, b'!'], None),
                (vec![0, 0, 0, 4, b'{'], None),
                (
                    ((crate::MAX_FRAME_BYTES as u32) + 1).to_be_bytes().to_vec(),
                    None,
                ),
                (duplicate.into_bytes(), Some(duplicate_len)),
                (
                    {
                        let mut trailing = valid.clone();
                        trailing.push(b'!');
                        trailing
                    },
                    Some(valid.len()),
                ),
            ];
            for (bytes, declared_length) in cases {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                if let Some(length) = declared_length {
                    server.write_all(&(length as u32).to_be_bytes()).unwrap();
                }
                server.write_all(&bytes).unwrap();
                server.shutdown(std::net::Shutdown::Write).unwrap();
                assert!(read_v2_reply(&mut client, &request, server_policy()).is_err());
            }
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn v2_reply_rejects_wrong_digest_ids_cursor_batch_and_version() {
            let request = sample_v2();
            let valid: Value =
                serde_json::from_slice(&v2_reply(&request).to_json(&request).unwrap()).unwrap();
            let mut altered = Vec::new();
            for (field, value) in [
                ("request_digest", json!("f".repeat(64))),
                ("request_id", json!("f".repeat(32))),
                ("receiver_id", json!("receiver:wrong")),
                ("version", json!(1)),
            ] {
                let mut changed = valid.clone();
                changed[field] = value;
                altered.push(changed);
            }
            for (field, value) in [
                ("cursor", json!("wrong-cursor")),
                ("batch_digest", json!("f".repeat(64))),
            ] {
                let mut changed = valid.clone();
                changed["result"][field] = value;
                altered.push(changed);
            }
            for changed in altered {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                write_frame_bytes(&mut server, &serde_json::to_vec(&changed).unwrap()).unwrap();
                server.shutdown(std::net::Shutdown::Write).unwrap();
                assert!(matches!(
                    read_v2_reply(&mut client, &request, server_policy()),
                    Err(ContractError::Invalid(_))
                ));
            }
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn v2_reply_has_one_total_frame_deadline() {
            let request = sample_v2();
            let (mut client, mut server) = UnixStream::pair().unwrap();
            server.write_all(&16_u32.to_be_bytes()).unwrap();
            server.write_all(b"x").unwrap();
            let producer = thread::spawn(move || {
                for _ in 0..8 {
                    thread::sleep(Duration::from_millis(50));
                    if server.write_all(b"x").is_err() {
                        break;
                    }
                }
            });
            assert!(matches!(
                read_v2_reply_with_timeout(
                    &mut client,
                    &request,
                    server_policy(),
                    Duration::from_millis(180)
                ),
                Err(ContractError::Io(ref error)) if error.kind() == ErrorKind::TimedOut
            ));
            producer.join().unwrap();
        }
    }
}

#[cfg(unix)]
pub use local::{
    AuthorizedRequest, AuthorizedV2Request, PeerCredentials, PeerPolicy, ServerPolicy,
    read_authorized_request, read_authorized_v2_request, read_probe_reply, read_v2_reply,
    send_probe_reply, send_request, send_v2_reply, send_v2_request,
};
