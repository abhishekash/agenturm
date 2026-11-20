//! Fixture-only v2 scratch client; never launched by the production receiver.

use agenturm_storage_service_contract::{
    MAX_FRAME_BYTES, ServerPolicy, read_v2_reply, send_v2_request, v2,
};
use std::env;
use std::error::Error;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let socket = arguments.next().ok_or("socket path required")?;
    let uid: u32 = arguments
        .next()
        .ok_or("service UID required")?
        .to_str()
        .ok_or("invalid service UID")?
        .parse()?;
    let gid: u32 = arguments
        .next()
        .ok_or("service GID required")?
        .to_str()
        .ok_or("invalid service GID")?
        .parse()?;
    if arguments.next().is_some() {
        return Err("unexpected client argument".into());
    }
    let mut raw = Vec::new();
    io::stdin()
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    let request = v2::Request::from_json(&raw)?;
    let policy = ServerPolicy::for_current_client(uid, gid)?;
    let mut stream = UnixStream::connect(Path::new(&socket))?;
    send_v2_request(&mut stream, &request, policy)?;
    let reply = read_v2_reply(&mut stream, &request, policy)?;
    io::stdout().write_all(&reply.to_json(&request)?)?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("v2 scratch client rejected: {error}");
        std::process::exit(1);
    }
}
