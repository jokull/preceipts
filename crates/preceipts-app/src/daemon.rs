//! A blocking, one-shot client for the daemon's control socket.
//!
//! The daemon's own client is async because the daemon is; this window is not,
//! and dragging tokio into it to send one JSON line would be a tail wagging a
//! dog. The protocol is a line in and a line out — `std::os::unix::net` says
//! that in thirty lines.
//!
//! Every call is blocking, so every call belongs on the background executor.
//! A wedged daemon must never be able to stop the window from painting, which
//! is what the timeouts below are for: no environment is a normal state, and
//! so is a daemon that has stopped answering.

use preceipts_proto::{Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Long enough for a daemon under load, short enough that a hung one shows as
/// "not answering" within one poll interval rather than never.
const TIMEOUT: Duration = Duration::from_secs(2);

pub fn call(socket: &Path, request: &Request) -> Result<Response, String> {
    if !socket.exists() {
        // Not an error worth decorating: a project with no environment up is
        // the ordinary case, and the panel says so in its own words.
        return Err("no daemon".to_string());
    }
    let stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();

    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    payload.push(b'\n');
    writer.write_all(&payload).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    // The daemon reads until EOF on its side before answering, so the write
    // half has to close. Shutting it down is what says "that was the whole
    // request" without inventing a length prefix.
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.trim().is_empty() {
        return Err("daemon closed the connection".to_string());
    }
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}
