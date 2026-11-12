//! A deliberately small HTTP/1.1 layer over `std::net`.
//!
//! The configurator serves one local user: a handful of JSON requests and one Server-Sent
//! Events stream. A hand-written layer (one thread per connection, `Connection: close`) keeps
//! the `web` feature free of server dependencies and lets the event stream flush every frame.
//! Limits: 16 KiB of headers, 1 MiB of body, no `Transfer-Encoding` request bodies.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use super::jobs::{Job, Wait};

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// Path without the query string.
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Builds a request for tests and in-process callers.
    pub fn new(method: &str, target: &str) -> Self {
        let (path, query) = split_target(target);
        Self {
            method: method.to_string(),
            path,
            query,
            ..Self::default()
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }
}

#[derive(Debug)]
pub enum ReadError {
    /// The peer closed before sending a request.
    Closed,
    Bad(&'static str),
    TooLarge,
    Io,
}

pub enum Body {
    Bytes(Vec<u8>),
    /// A Server-Sent Events stream over a benchmark job's log.
    Events(Arc<Job>),
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    /// Stop the server once this response has been written.
    pub then_shutdown: bool,
}

impl Response {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type".into(), content_type.into()),
                ("Cache-Control".into(), "no-store".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body: Body::Bytes(body),
            then_shutdown: false,
        }
    }

    pub fn json<T: serde::Serialize>(status: u16, value: &T) -> Self {
        let body = serde_json::to_vec(value).unwrap_or_else(|_| b"null".to_vec());
        Self::new(status, "application/json", body)
    }

    pub fn html(body: &'static str) -> Self {
        let mut r = Self::new(200, "text/html; charset=utf-8", body.as_bytes().to_vec());
        r.headers.push((
            "Content-Security-Policy".into(),
            "default-src 'self'; style-src 'unsafe-inline'; script-src 'self' 'unsafe-inline'"
                .into(),
        ));
        r
    }

    pub fn redirect(location: &str) -> Self {
        let mut r = Self::new(302, "text/plain", Vec::new());
        r.headers.push(("Location".into(), location.into()));
        r
    }

    pub fn empty(status: u16) -> Self {
        Self::new(status, "text/plain", Vec::new())
    }

    pub fn events(job: Arc<Job>) -> Self {
        let mut r = Self::new(200, "text/event-stream", Vec::new());
        r.body = Body::Events(job);
        r
    }

    pub fn with_header(mut self, name: &str, value: String) -> Self {
        self.headers.push((name.to_string(), value));
        self
    }

    pub fn status_text(&self) -> &'static str {
        match self.status {
            200 => "OK",
            202 => "Accepted",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            422 => "Unprocessable Entity",
            _ => "Error",
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn split_target(target: &str) -> (String, Vec<(String, String)>) {
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let query = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    (path.to_string(), query)
}

/// Reads one request from `stream`. Answers `Expect: 100-continue` itself.
pub fn read_request(stream: &mut TcpStream) -> Result<Request, ReadError> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|_| ReadError::Io)?;
    let mut reader = BufReader::new(&mut *stream);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take((MAX_HEADERS - head.len() + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| ReadError::Io)?;
        if n == 0 {
            return Err(if head.is_empty() {
                ReadError::Closed
            } else {
                ReadError::Bad("truncated request")
            });
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEADERS {
            return Err(ReadError::TooLarge);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let text = String::from_utf8(head).map_err(|_| ReadError::Bad("headers are not UTF-8"))?;
    let mut lines = text.lines();
    let first = lines.next().ok_or(ReadError::Bad("empty request"))?;
    let mut parts = first.split_whitespace();
    let (method, target, version) = (
        parts.next().ok_or(ReadError::Bad("no method"))?,
        parts.next().ok_or(ReadError::Bad("no target"))?,
        parts.next().ok_or(ReadError::Bad("no version"))?,
    );
    if !version.starts_with("HTTP/1.") {
        return Err(ReadError::Bad("unsupported HTTP version"));
    }
    let mut req = Request::new(method, target);
    for l in lines.take_while(|l| !l.is_empty()) {
        let (k, v) = l
            .split_once(':')
            .ok_or(ReadError::Bad("malformed header"))?;
        req.headers
            .push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    if req.header("transfer-encoding").is_some() {
        return Err(ReadError::Bad("chunked request bodies are not supported"));
    }
    let len: usize = match req.header("content-length") {
        Some(v) => v
            .parse()
            .map_err(|_| ReadError::Bad("bad Content-Length"))?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(ReadError::TooLarge);
    }
    if len > 0 {
        if req
            .header("expect")
            .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
        {
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .map_err(|_| ReadError::Io)?;
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).map_err(|_| ReadError::Io)?;
        req.body = body;
    }
    Ok(req)
}

/// Writes `resp` and closes. An event stream is flushed frame by frame until its job ends.
pub fn write_response(stream: &mut TcpStream, resp: &Response) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, resp.status_text());
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n");
    match &resp.body {
        Body::Bytes(b) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", b.len()));
            stream.write_all(head.as_bytes())?;
            stream.write_all(b)?;
            stream.flush()
        }
        Body::Events(job) => {
            head.push_str("\r\n");
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            stream.set_write_timeout(Some(Duration::from_secs(10)))?;
            let mut next = 0;
            loop {
                match job.wait(next, Duration::from_secs(15)) {
                    Wait::Event(ev) => {
                        next += 1;
                        let data = serde_json::to_string(&ev).unwrap_or_default();
                        write!(stream, "event: {}\ndata: {data}\n\n", ev.name())?;
                    }
                    Wait::Idle => stream.write_all(b": keepalive\n\n")?,
                    Wait::Done => return Ok(()),
                }
                stream.flush()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_split_into_path_and_decoded_query() {
        let r = Request::new("GET", "/a/b?token=ab%20c&x=1+2&flag");
        assert_eq!(r.path, "/a/b");
        assert_eq!(r.query("token"), Some("ab c"));
        assert_eq!(r.query("x"), Some("1 2"));
        assert_eq!(r.query("flag"), Some(""));
        assert_eq!(r.query("none"), None);
    }

    #[test]
    fn a_bad_escape_stays_literal() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%41"), "A");
    }

    #[test]
    fn headers_are_found_case_insensitively_by_lower_name() {
        let r = Request::new("GET", "/").with_header("Origin", "x");
        assert_eq!(r.header("origin"), Some("x"));
    }
}
