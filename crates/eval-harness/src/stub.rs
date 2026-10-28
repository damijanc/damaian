//! A loopback HTTP stub for the fetch scenarios (spec 51 Task 9).
//!
//! The deterministic tier must not reach the internet, but a fetch scenario
//! needs a page to fetch. This serves a scenario's committed pages from
//! `127.0.0.1` on an **ephemeral** port, so the URL the model sees is
//! substituted at run time and never names a real host. The server is owned by
//! the run and shut down when the run ends; nothing outlives the process.
//!
//! It deliberately does not use port 4765, the desktop shell's port. A user's
//! Damaian instance may be listening there, and a test must never take it over.
//!
//! It is not `#[ignore]`d for the same reason the engine's loopback stub is not:
//! the port is ephemeral, the listener is bound to `127.0.0.1` only, it is shut
//! down and joined when the run ends, and no external host is contacted.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use workspace_engine::{ClientError, Result};

use crate::scenario::StubPage;

const LOOPBACK: &str = "127.0.0.1";
const FORBIDDEN_PORT: u16 = 4765;
const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;

/// One running stub server. Dropping it stops the accept loop and joins the
/// thread, so a run cannot leave a listener behind.
pub struct StubServer {
    base: String,
    approval_key: String,
    requests: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl StubServer {
    /// Starts a server for the scenario's pages, or `None` when it declares
    /// none. The server is bound before this returns, so `base` is usable
    /// immediately.
    pub fn start(pages: &[StubPage]) -> Result<Option<Self>> {
        if pages.is_empty() {
            return Ok(None);
        }
        let listener = bind_ephemeral()?;
        let address = listener.local_addr().map_err(io("read the stub address"))?;
        listener
            .set_nonblocking(true)
            .map_err(io("make the stub listener non-blocking"))?;

        let pages = Arc::new(pages.to_vec());
        let requests = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("eval-stub-page".to_string())
            .spawn(move || serve(listener, &pages, &thread_requests, &thread_shutdown))
            .map_err(io("start the stub server"))?;

        Ok(Some(Self {
            base: format!("http://{address}"),
            approval_key: address.to_string(),
            requests,
            shutdown,
            handle: Some(handle),
        }))
    }

    /// The URL prefix a scenario writes as `{stub_base}`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The `host:port` a user-scope allowlist entry needs. The port is
    /// non-default, so `ExternalUrl::approval_key` keeps it and an entry
    /// naming only the host would not match.
    pub fn approval_key(&self) -> &str {
        &self.approval_key
    }

    /// How many connections the server accepted. One per request the engine
    /// actually made, which is what a refusal scenario asserts is zero.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Binds `127.0.0.1:0`, retrying if the kernel ever offers the shell's port.
fn bind_ephemeral() -> Result<TcpListener> {
    for _ in 0..16 {
        let listener = TcpListener::bind((LOOPBACK, 0)).map_err(io("bind the stub server"))?;
        let port = listener
            .local_addr()
            .map_err(io("read the stub port"))?
            .port();
        if port != FORBIDDEN_PORT {
            return Ok(listener);
        }
        // Never occupy the port the user's desktop shell may be using. The
        // listener is dropped at the end of this iteration and a new ephemeral
        // port is requested.
    }
    Err(ClientError::InvalidInput(format!(
        "could not bind the stub server away from port {FORBIDDEN_PORT}"
    )))
}

fn serve(listener: TcpListener, pages: &[StubPage], requests: &AtomicU64, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                requests.fetch_add(1, Ordering::SeqCst);
                let _ = respond(stream, pages);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // A non-blocking listener has no wake-up, so poll. Two
                // milliseconds is short enough that a scenario's fetch never
                // waits on it and long enough not to spin a core.
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn respond(mut stream: TcpStream, pages: &[StubPage]) -> std::io::Result<()> {
    // Accepted sockets' blocking mode is not portable across platforms even
    // when the listener is non-blocking, so set both explicitly.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;

    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while head.len() <= MAX_REQUEST_HEAD_BYTES {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&buffer[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&head);
    let target = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    // Pages are keyed by path. The query is ignored on purpose: a scenario
    // counts requests to an endpoint it expects the egress guard to refuse, so
    // matching it would only make a failed guard harder to read.
    let path = target.split('?').next().unwrap_or("/");
    let page = pages.iter().find(|page| page.path == path);
    let (status, content_type, body, location) = match page {
        Some(page) => (
            page.status,
            page.content_type.as_str(),
            page.body.as_str(),
            page.location.as_deref(),
        ),
        None => (404, "text/plain; charset=utf-8", "no such stub page", None),
    };

    let mut response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reason(status),
        body.len()
    );
    if let Some(location) = location {
        response.push_str("Location: ");
        response.push_str(location);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    response.push_str(body);

    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Response",
    }
}

fn io(action: &'static str) -> impl Fn(std::io::Error) -> ClientError {
    move |error| ClientError::Io(format!("{action}: {error}"))
}
