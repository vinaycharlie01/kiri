//! Just enough HTTP/1.1 over a Unix socket to speak Docker's Engine API to
//! Kiri's daemon: one short connection per request (`Connection: close`),
//! Content-Length, chunked and read-to-close bodies, and a long-lived chunked
//! stream for `/events`.
//!
//! This mirrors `crates/kiri/src/client.rs`, which lives inside the `kiri`
//! binary crate and so cannot be depended on. Extracting a shared client crate
//! upstream is the better end state; see docs/ai-platform/DEPENDENCY_MAP.md.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::ops::ControlFlow;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// The largest body this client reads. The engine is local and trusted
/// infrastructure, but a bound keeps a fault from exhausting memory.
const MAX_BODY: usize = 64 << 20;
/// Longest status or header line accepted.
const MAX_LINE: usize = 16 << 10;
/// Long enough for `docker stop`'s default 10 s grace period and then some.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The engine's `{"message": …}`, or the body as text.
    pub fn message(&self) -> String {
        match self.json()["message"].as_str() {
            Some(m) => m.to_owned(),
            None => String::from_utf8_lossy(&self.body).trim().to_owned(),
        }
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

fn read_line(r: &mut impl BufRead, line: &mut String) -> io::Result<usize> {
    line.clear();
    let n = r.by_ref().take(MAX_LINE as u64 + 1).read_line(line)?;
    if n > MAX_LINE {
        return Err(bad("header line too long"));
    }
    Ok(n)
}

/// Status, Content-Length and whether the body is chunked.
fn head(r: &mut impl BufRead) -> io::Result<(u16, Option<usize>, bool)> {
    let mut line = String::new();
    read_line(r, &mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("bad status line"))?;
    let (mut len, mut chunked) = (None, false);
    loop {
        if read_line(r, &mut line)? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = Some(v.trim().parse().map_err(|_| bad("bad content-length"))?);
            } else if k.eq_ignore_ascii_case("transfer-encoding") {
                chunked = v.trim().eq_ignore_ascii_case("chunked");
            }
        }
    }
    Ok((status, len, chunked))
}

/// One chunk of a chunked body; `None` at the end.
fn chunk(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = String::new();
    if read_line(r, &mut line)? == 0 {
        return Ok(None);
    }
    let size = line.trim().split(';').next().unwrap_or("");
    let n = usize::from_str_radix(size, 16).map_err(|_| bad("bad chunk size"))?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_BODY {
        return Err(bad("chunk too large"));
    }
    let mut buf = vec![0; n + 2];
    r.read_exact(&mut buf)?;
    buf.truncate(n);
    Ok(Some(buf))
}

fn connect(socket: &Path, timeout: Option<Duration>) -> io::Result<UnixStream> {
    let s = UnixStream::connect(socket)?;
    s.set_read_timeout(timeout)?;
    s.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    Ok(s)
}

/// One request without a body; the whole response body is read.
pub fn request(socket: &Path, method: &str, path: &str) -> io::Result<Response> {
    let mut s = connect(socket, Some(REQUEST_TIMEOUT))?;
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: kiri\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )?;
    let mut r = BufReader::new(s);
    let (status, len, chunked) = head(&mut r)?;
    let mut body = Vec::new();
    // 204 and 304 never have a body, and the server may keep the connection.
    if status == 204 || status == 304 || method == "HEAD" {
        return Ok(Response { status, body });
    }
    if chunked {
        while let Some(c) = chunk(&mut r)? {
            if body.len() + c.len() > MAX_BODY {
                return Err(bad("response too large"));
            }
            body.extend_from_slice(&c);
        }
    } else if let Some(n) = len {
        if n > MAX_BODY {
            return Err(bad("response too large"));
        }
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    } else {
        r.take(MAX_BODY as u64 + 1).read_to_end(&mut body)?;
        if body.len() > MAX_BODY {
            return Err(bad("response too large"));
        }
    }
    Ok(Response { status, body })
}

/// A GET whose chunked answer is newline-delimited JSON arriving over time
/// (`/events`). Calls `on_line` per complete line until it breaks or the
/// stream ends. A non-200 answer is returned as a `Response` instead.
pub fn stream_lines(
    socket: &Path,
    path: &str,
    on_line: &mut dyn FnMut(&[u8]) -> ControlFlow<()>,
) -> io::Result<Option<Response>> {
    let mut s = connect(socket, None)?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: kiri\r\nConnection: close\r\n\r\n"
    )?;
    let mut r = BufReader::new(s);
    let (status, len, chunked) = head(&mut r)?;
    if status != 200 {
        let mut body = Vec::new();
        match len {
            Some(n) if n <= MAX_BODY => {
                body.resize(n, 0);
                r.read_exact(&mut body)?;
            }
            _ => {}
        }
        return Ok(Some(Response { status, body }));
    }
    if !chunked {
        return Err(bad("event stream is not chunked"));
    }
    let mut pending = Vec::new();
    while let Some(c) = chunk(&mut r)? {
        pending.extend_from_slice(&c);
        while let Some(i) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=i).collect();
            if on_line(&line).is_break() {
                return Ok(None);
            }
        }
        if pending.len() > MAX_LINE * 64 {
            return Err(bad("event line too long"));
        }
    }
    Ok(None)
}
