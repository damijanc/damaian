//! The one request a `fetch_url` call makes (spec 51 §5.3, D3, D12).
//!
//! This is deliberately not [`crate::model::CurlModelTransport`]. That transport
//! exists to send a model provider's key to a configured base URL, and the one
//! thing a fetch must never do is send that key anywhere. A separate type with
//! no credential field at all makes the mistake unrepresentable rather than
//! merely forbidden: there is nothing here to copy a secret into, and the curl
//! invocation below carries a fixed user agent and an `Accept` header and
//! nothing else.
//!
//! Three other rules live here because they are properties of the request:
//!
//! - **No redirect is followed, of any kind** (D10). `curl` is invoked without
//!   any redirect-following option, and a 3xx is classified through
//!   [`crate::external_reference::redirect_target`], whose `may_follow()` is
//!   always false.
//! - **The body is bounded while it arrives.** `--max-filesize` alone is not a
//!   bound against a server that sends no `Content-Length`, so the reader stops
//!   at the cap and kills the child (D11, §5.5).
//! - **Stored text is redacted.** The body is scanned with [`SecretScanner`]
//!   before it is cached or returned, because a documentation page often
//!   contains a key-shaped sample and an unredacted one would reach the session
//!   log, the transcript and a later report (§5.5).
//!
//! The body is untrusted data. It is decoded lossily, redacted and returned to
//! the caller; nothing here executes or interprets it, and it is never logged.
//! A response whose content type §5.5 does not convert is withheld here — its
//! body is cleared and it is never cached — because this is the last point
//! before a binary body could be stored or handed on as text. Reducing HTML to
//! text and the second redaction belong to
//! [`crate::external_reference::ConvertedResponse`].

use crate::cancel::CancelToken;
use crate::error::{ClientError, Result};
use crate::external_reference::{
    ContentKind, ExternalUrl, RedirectTarget, is_public_address, redirect_target,
};
use crate::hash::{create_id, now_millis, sha256, sha256_hex};
use crate::model::escape_curl_config_value;
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// The response cap: a fetched page is reference material, not a dataset. 200
/// KiB matches the size rule in D11 and is enough for a documentation page.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 200 * 1024;

/// How long the fetch may spend reaching the host before giving up.
const CONNECT_TIMEOUT_SECS: u64 = 15;
/// How long a connected response may deliver effectively nothing (under one
/// byte per second) before it is treated as wedged.
const STALL_TIMEOUT_SECS: u64 = 30;
/// The total ceiling for one fetch. A page that has not arrived by then is not
/// worth the turn.
const MAX_TIME_SECS: u64 = 60;
/// How often the blocked reader re-checks the cancellation flag.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The whole of Damaian's identity in the request. Not a product URL: the
/// request should name the tool, not invite tracking of an install.
const USER_AGENT: &str = "Damaian";
/// The content types §5.5 converts, asked for up front. A server may answer with
/// anything; Task 5 refuses the types it cannot convert.
const ACCEPT: &str = "text/*, application/json, application/xhtml+xml";

/// What one fetch produced. `status`, `content_type`, `bytes`, `truncated` and
/// `redirect_to` are the facts a tool result, an audit event and the context
/// inspector need; `body` is the redacted, bounded text itself.
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    /// The HTTP status, from the response's last header block.
    pub status: u16,
    /// The `content-type` header verbatim, parameters included. `None` when the
    /// server sent none.
    pub content_type: Option<String>,
    /// The response's retained size: what was read after the cap and redaction,
    /// not the server's declared length. For a withheld non-text body this is
    /// the size the caller reports, and `body` is empty.
    pub bytes: usize,
    pub truncated: bool,
    /// Why the body is not the whole response, when it is not.
    pub truncation_reason: Option<String>,
    /// The target of a 3xx, named and never followed (D10).
    pub redirect_to: Option<RedirectTarget>,
    /// Redacted, bounded text. The caller must treat this as data. Empty when
    /// [`Self::body_withheld`] is true.
    pub body: String,
    /// True when the content type is not one §5.5 converts, so the body was
    /// cleared before it could be cached or returned as text.
    pub body_withheld: bool,
    /// When the response was fetched. A cache hit reports the original fetch,
    /// so a caller can tell how old the page is.
    pub fetched_at_ms: u64,
    /// True when this came from the session's cache rather than a request.
    pub cached: bool,
}

impl FetchOutcome {
    pub fn is_redirect(&self) -> bool {
        self.redirect_to.is_some()
    }
}

/// A hostname resolved to an address that passed the public-address check, and
/// the one address curl is pinned to. Pinning is what makes the check and the
/// connection agree: without `--resolve`, curl would resolve the name a second
/// time and a DNS answer could change in between.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedTarget {
    host: String,
    port: u16,
    address: IpAddr,
    /// An address literal in the URL needs no `--resolve`; curl connects to it
    /// directly and the literal itself was already validated.
    literal: bool,
}

impl PinnedTarget {
    /// The `--resolve` value. An IPv6 address is bracketed, as curl requires.
    fn resolve_value(&self) -> Option<String> {
        if self.literal {
            return None;
        }
        let address = match self.address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        Some(format!("{}:{}:{address}", self.host, self.port))
    }
}

/// The fetch transport: one credential-free `curl` invocation per request, and a
/// session-scoped cache of what came back.
pub struct CurlFetchTransport {
    /// So a `curl` in flight when the process is killed is swept later.
    registry: ProcessRegistry,
    /// The root the `tmp` header files and the `external` cache live under.
    data_dir: PathBuf,
    /// The session this fetch belongs to. The cache is keyed by it, so a stale
    /// page can never be presented to another session as current (D12).
    session_id: String,
    /// Applied before a body is cached or returned (§5.5).
    scanner: SecretScanner,
    max_bytes: usize,
    /// Whether a non-public destination is permitted. False everywhere in the
    /// product; the eval harness and the engine's own tests turn it on so a
    /// `127.0.0.1` stub can be fetched. No config field can reach it, so a
    /// repository or user approve-list entry cannot widen it.
    allow_non_public: bool,
}

impl CurlFetchTransport {
    pub fn new(
        registry: ProcessRegistry,
        data_dir: impl AsRef<Path>,
        session_id: impl Into<String>,
        scanner: SecretScanner,
    ) -> Self {
        let data_dir = data_dir.as_ref().to_path_buf();
        // Best-effort: a fetch into an unwritable directory fails honestly when
        // curl cannot write its header file, and a missing cache is a miss, not
        // an error.
        let _ = fs::create_dir_all(data_dir.join("tmp"));
        Self {
            registry,
            data_dir,
            session_id: session_id.into(),
            scanner,
            max_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            allow_non_public: false,
        }
    }

    /// The response cap, for a test or a future config key. Zero is never a
    /// limit here: `curl` reads `--max-filesize 0` as "no limit", which is the
    /// opposite of what the caller asked for.
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Permit a non-public destination. Called only by the engine's tests and
    /// the eval harness, whose stub binds `127.0.0.1`; deliberately a
    /// constructor step rather than a `Config` field, so neither repository nor
    /// user config can enable it.
    #[doc(hidden)]
    pub fn with_non_public_allowed_for_tests(mut self) -> Self {
        self.allow_non_public = true;
        self
    }

    /// The curl arguments. `-q` keeps a user's `~/.curlrc` out of a request whose
    /// headers are a security promise, and `-g` turns off URL globbing: a path
    /// containing `[1-3]` would otherwise become three requests. `--noproxy *`
    /// keeps the user's proxy environment out of a fetch, so neither a stub nor
    /// a real page is routed through an intermediary. No redirect-following
    /// argument appears here, and the test suite asserts that absence against
    /// the source.
    pub fn curl_args() -> [&'static str; 8] {
        [
            "-q",
            "-g",
            "-sS",
            "--no-buffer",
            "--noproxy",
            "*",
            "--config",
            "-",
        ]
    }

    /// The proxy variables removed from the child. `--noproxy *` already stops
    /// curl using a proxy; removing the variables as well means the rule holds
    /// even if a future argument change removes that flag.
    pub fn proxy_environment_variables() -> [&'static str; 8] {
        [
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
        ]
    }

    /// The curl invocation. Public for the test that asserts no proxy
    /// configuration reaches the child; production always goes through
    /// [`Self::fetch`].
    #[doc(hidden)]
    pub fn curl_command() -> Command {
        let mut command = Command::new("curl");
        command.args(Self::curl_args());
        for variable in Self::proxy_environment_variables() {
            command.env_remove(variable);
        }
        command
    }

    /// The request as a curl config file, written to curl's stdin. The URL is
    /// [`ExternalUrl::request_url`], so the fragment is already gone; the only
    /// headers are the fixed user agent and `Accept`. `pinned` adds the
    /// `--resolve` line that keeps the connection to the validated address.
    pub fn curl_config(
        &self,
        url: &ExternalUrl,
        header_path: &Path,
        pinned: Option<&PinnedTarget>,
    ) -> String {
        let resolve = pinned
            .and_then(PinnedTarget::resolve_value)
            .map(|value| format!("resolve = \"{}\"\n", escape_curl_config_value(&value)))
            .unwrap_or_default();
        format!(
            "request = \"GET\"\nurl = \"{}\"\nheader = \"user-agent: {USER_AGENT}\"\nheader = \"accept: {ACCEPT}\"\ndump-header = \"{}\"\n{resolve}connect-timeout = {CONNECT_TIMEOUT_SECS}\nspeed-limit = 1\nspeed-time = {STALL_TIMEOUT_SECS}\nmax-time = {MAX_TIME_SECS}\nmax-filesize = {}\n",
            escape_curl_config_value(&url.request_url()),
            escape_curl_config_value(&header_path.display().to_string()),
            self.max_bytes
        )
    }

    /// Fetch `url`, or serve the same session's earlier copy of it.
    ///
    /// A cache hit is returned before anything is spawned, so a repeat fetch
    /// makes no request at all. Only this session's directory is ever read.
    pub fn fetch(&self, url: &ExternalUrl, cancel: &CancelToken) -> Result<FetchOutcome> {
        cancel.check()?;
        if let Some(cached) = self.read_cache(url) {
            return Ok(withhold_unless_convertible(cached));
        }
        let outcome = self.fetch_uncached(url, cancel)?;
        // Before the write: a binary body must never reach the cache, and a
        // caller must never receive one as text.
        let outcome = withhold_unless_convertible(outcome);
        self.write_cache(url, &outcome);
        Ok(outcome)
    }

    fn fetch_uncached(&self, url: &ExternalUrl, cancel: &CancelToken) -> Result<FetchOutcome> {
        // Resolve before anything is spawned, so a private destination is
        // refused without a request and the connection can be pinned to the
        // address that was validated (§5.3).
        let pinned = resolve_target(url, self.allow_non_public)?;
        let header_path = self
            .data_dir
            .join("tmp")
            .join(format!("{}.headers", create_id("fetch")));
        let _header_guard = HeaderFileGuard {
            path: header_path.clone(),
        };

        let spawned = Self::curl_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so a sweep reaches the whole call rather than a
            // leader that may have exec'd.
            .process_group(0)
            .spawn()?;
        // Before the request goes out: a running, unrecorded curl can be left
        // behind by a crash, and this one has already left the machine.
        let registration =
            self.registry
                .register(ProcessKind::Fetch, &self.session_id, spawned.id())?;
        let mut child = KillOnDrop {
            child: spawned,
            _registration: registration,
        };

        if let Some(mut stdin) = child.child().stdin.take() {
            stdin.write_all(
                self.curl_config(url, &header_path, Some(&pinned))
                    .as_bytes(),
            )?;
        }

        // Taken out first so the borrow for the scrutinee ends before the
        // cancellation closure borrows the child to kill it.
        let stdout = child.child().stdout.take();
        let (body, truncated) = match stdout {
            Some(stdout) => pump_body(stdout, cancel, self.max_bytes, || {
                // Closes the pipe, which lets the reader thread finish so
                // `pump_body` can join it instead of hanging.
                let _ = child.child().kill();
            })?,
            None => (Vec::new(), false),
        };

        let status = child.child().wait()?;
        let mut stderr = String::new();
        if let Some(mut stderr_pipe) = child.child().stderr.take() {
            stderr_pipe.read_to_string(&mut stderr)?;
        }

        // Read before the guard removes the file, so the status, type and
        // redirect survive the call.
        let headers = fs::read_to_string(&header_path)
            .map(|dump| parse_response_headers(&dump))
            .unwrap_or_default();

        // curl's own `max-filesize` guard fires with exit 63 when the server
        // declares a body over the cap. That is the same truncation the pump
        // reports, just decided earlier, so it is a result rather than an error.
        let declared_too_large = status.code() == Some(FILESIZE_EXCEEDED);
        let truncated = truncated || declared_too_large;
        if !status.success() && !truncated {
            let stderr = stderr.trim();
            return Err(ClientError::Io(if stderr.is_empty() {
                format!("fetch failed: curl exited with {status}")
            } else {
                format!("fetch failed: {stderr}")
            }));
        }

        let http_status = headers
            .status
            .ok_or_else(|| ClientError::Io("fetch failed: curl reported no HTTP status".into()))?;
        let body = self.scanner.redact(&String::from_utf8_lossy(&body)).text;
        let redirect_to = if (300..400).contains(&http_status) {
            headers
                .location
                .as_deref()
                .map(|location| redirect_target(url, location))
        } else {
            None
        };
        let truncation_reason = truncation_reason(truncated, declared_too_large, self.max_bytes);

        Ok(FetchOutcome {
            status: http_status,
            content_type: headers.content_type,
            bytes: body.len(),
            truncated,
            truncation_reason,
            redirect_to,
            body,
            body_withheld: false,
            fetched_at_ms: now_millis() as u64,
            cached: false,
        })
    }

    /// `<data_dir>/external/<session-id>/`, and `None` when the session id
    /// cannot name a directory. A transport with no session (an internal call
    /// that is not part of a conversation) therefore keeps no cache.
    fn cache_dir(&self) -> Option<PathBuf> {
        let session = self.session_id.trim();
        if session.is_empty() || session == "." || session == ".." || session.contains(['/', '\\'])
        {
            return None;
        }
        Some(self.data_dir.join("external").join(session))
    }

    fn cache_path(&self, url: &ExternalUrl) -> Option<PathBuf> {
        let dir = self.cache_dir()?;
        Some(dir.join(format!("{}.json", sha256_hex(url.request_url().as_bytes()))))
    }

    /// A cache hit is used only when it names the same URL and its stored body
    /// still hashes to the recorded digest. A miss, a torn record or a modified
    /// file all mean "fetch it again", never "serve something we cannot vouch
    /// for".
    fn read_cache(&self, url: &ExternalUrl) -> Option<FetchOutcome> {
        let path = self.cache_path(url)?;
        let text = fs::read_to_string(path).ok()?;
        let record: CachedFetch = serde_json::from_str(&text).ok()?;
        if record.url != url.request_url() {
            return None;
        }
        if sha256(record.body.as_bytes()) != record.content_hash {
            return None;
        }
        Some(FetchOutcome {
            status: record.status,
            content_type: record.content_type,
            bytes: record.body.len(),
            truncated: record.truncated,
            truncation_reason: record.truncation_reason,
            redirect_to: None,
            body: record.body,
            body_withheld: false,
            fetched_at_ms: record.fetched_at_ms,
            cached: true,
        })
    }

    /// Best-effort: a cache that cannot be written is a missed optimisation, not
    /// a failed fetch. Redirects are never stored, because there is no content
    /// to serve and the next call must be a fresh decision.
    fn write_cache(&self, url: &ExternalUrl, outcome: &FetchOutcome) {
        if outcome.is_redirect() || outcome.body_withheld {
            return;
        }
        let Some(path) = self.cache_path(url) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let record = CachedFetch {
            url: url.request_url(),
            fetched_at_ms: outcome.fetched_at_ms,
            status: outcome.status,
            content_hash: sha256(outcome.body.as_bytes()),
            content_type: outcome.content_type.clone(),
            truncated: outcome.truncated,
            truncation_reason: outcome.truncation_reason.clone(),
            body: outcome.body.clone(),
        };
        if let Ok(json) = serde_json::to_string(&record) {
            let _ = fs::write(path, json);
        }
    }
}

/// Resolve `url` to one address curl may connect to, refusing every non-public
/// answer (§5.3).
///
/// Every resolved address is checked, not just the one that will be used: a
/// name with both a public and a private answer is a rebinding attempt, and
/// picking the public one would leave the window open on a later query. The
/// first address is pinned with `--resolve`, so curl does not resolve again.
fn resolve_target(url: &ExternalUrl, allow_non_public: bool) -> Result<PinnedTarget> {
    let port = url.port.unwrap_or_else(|| url.scheme.default_port());
    let literal = url.host.parse::<IpAddr>().ok();
    let addresses: Vec<SocketAddr> = match literal {
        Some(address) => vec![SocketAddr::new(address, port)],
        None => (url.host.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| {
                ClientError::Io(format!(
                    "fetch failed: could not resolve {}: {error}",
                    url.host
                ))
            })?
            .collect(),
    };
    if addresses.is_empty() {
        return Err(ClientError::Io(format!(
            "fetch failed: {} resolved to no address",
            url.host
        )));
    }
    if !allow_non_public
        && let Some(refused) = addresses
            .iter()
            .find(|address| !is_public_address(address.ip()))
    {
        return Err(ClientError::PolicyBlocked(format!(
            "fetch failed: {} resolves to {}, which is not a public internet address",
            url.host,
            refused.ip()
        )));
    }
    Ok(PinnedTarget {
        host: url.host.clone(),
        port,
        address: addresses[0].ip(),
        literal: literal.is_some(),
    })
}

/// curl's exit code for "maximum file size exceeded".
const FILESIZE_EXCEEDED: i32 = 63;

/// Clear the body of a response §5.5 does not convert, keeping its size.
///
/// A redirect carries no content and is left alone; a 200 whose type is
/// `application/octet-stream` (or is missing) keeps its recorded size and loses
/// its bytes. This is the gate the cache and the caller both sit behind.
fn withhold_unless_convertible(mut outcome: FetchOutcome) -> FetchOutcome {
    if outcome.is_redirect()
        || ContentKind::classify(outcome.content_type.as_deref()).is_supported()
    {
        return outcome;
    }
    outcome.body_withheld = true;
    outcome.body.clear();
    outcome
}

fn truncation_reason(
    truncated: bool,
    declared_too_large: bool,
    max_bytes: usize,
) -> Option<String> {
    if !truncated {
        return None;
    }
    Some(if declared_too_large {
        format!(
            "the server declared a response larger than the {max_bytes}-byte cap, so it was not read"
        )
    } else {
        format!("the response was over the {max_bytes}-byte cap and was truncated")
    })
}

/// What a curl `dump-header` file says. A proxy `CONNECT` or a `100 Continue`
/// writes more than one block; the last one is the response whose body we read,
/// so each new status line resets the fields.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ResponseHeaders {
    status: Option<u16>,
    content_type: Option<String>,
    location: Option<String>,
}

fn parse_response_headers(dump: &str) -> ResponseHeaders {
    let mut headers = ResponseHeaders::default();
    for line in dump.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HTTP/") {
            headers.status = line
                .split_whitespace()
                .nth(1)
                .and_then(|status| status.parse().ok());
            headers.content_type = None;
            headers.location = None;
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("content-type") {
            headers.content_type = Some(value.to_string());
        } else if name.trim().eq_ignore_ascii_case("location") {
            headers.location = Some(value.to_string());
        }
    }
    headers
}

/// Reads `reader` to the cap, handing nothing to anyone else, and gives up as
/// soon as `cancel` is set or the cap is crossed.
///
/// The blocking read runs on its own thread because it offers no chance to
/// notice either event; the calling thread waits on the channel with a timeout
/// instead. `on_stop` runs before the join and must make the reader finish — for
/// a child process, by killing it — so the join is never a wait for the rest of
/// the response.
fn pump_body<R>(
    reader: R,
    cancel: &CancelToken,
    max_bytes: usize,
    on_stop: impl FnOnce(),
) -> Result<(Vec<u8>, bool)>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut body = Vec::new();
    let mut truncated = false;
    let outcome = loop {
        if cancel.is_cancelled() {
            break Err(ClientError::Cancelled);
        }
        match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(chunk) => {
                let room = max_bytes.saturating_sub(body.len());
                if chunk.len() > room {
                    body.extend_from_slice(&chunk[..room]);
                    truncated = true;
                    break Ok(());
                }
                body.extend_from_slice(&chunk);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }
    };

    if outcome.is_err() || truncated {
        on_stop();
    }
    let _ = reader_thread.join();
    outcome.map(|()| (body, truncated))
}

/// The stored form of one session's response. `contentType`, `truncated` and
/// `truncationReason` are carried so a cache hit can be gated and labelled
/// exactly as a fresh fetch; without them a repeat fetch would silently lose
/// the facts §5.5 requires the result to state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedFetch {
    url: String,
    fetched_at_ms: u64,
    status: u16,
    content_hash: String,
    body: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    truncation_reason: Option<String>,
}

/// Removes the `dump-header` file on every exit path, so a crashed or cancelled
/// fetch does not leave response headers on disk.
struct HeaderFileGuard {
    path: PathBuf,
}

impl Drop for HeaderFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Kills the child if it is still running when this is dropped, so a cancelled
/// or panicked fetch cannot leave curl reading a response nobody will use. The
/// handle also removes the registry entry on the same path; a `SIGKILL` skips
/// this, which is why the entry is written at spawn.
struct KillOnDrop {
    child: Child,
    /// Held, never read: dropping it is what removes the registry entry.
    _registration: RegistrationHandle,
}

impl KillOnDrop {
    fn child(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_header_block_is_the_response_that_was_read() {
        // A proxy CONNECT or an interim `100 Continue` writes a block first;
        // taking the first status line would report 100 as the answer.
        let dump = "HTTP/1.1 200 Connection established\r\n\r\nHTTP/1.1 302 Found\r\ncontent-type: text/html; charset=utf-8\r\nlocation: https://docs.rs/other\r\n\r\n";
        let headers = parse_response_headers(dump);
        assert_eq!(headers.status, Some(302));
        assert_eq!(
            headers.content_type.as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(headers.location.as_deref(), Some("https://docs.rs/other"));

        assert_eq!(
            parse_response_headers("garbage"),
            ResponseHeaders::default()
        );
    }

    #[test]
    fn header_names_are_matched_case_insensitively() {
        let headers = parse_response_headers("HTTP/2 200\r\nContent-Type: application/json\r\n");
        assert_eq!(headers.status, Some(200));
        assert_eq!(headers.content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn a_resolved_name_is_pinned_in_the_request() {
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-fetch-pin-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let transport = CurlFetchTransport::new(
            ProcessRegistry::open(&data_dir).expect("registry"),
            &data_dir,
            "session_pin",
            SecretScanner::new(Vec::new()),
        );
        let url = ExternalUrl::parse("https://docs.example.com/guide").expect("a valid URL");
        let pinned = PinnedTarget {
            host: "docs.example.com".to_string(),
            port: 443,
            address: "93.184.216.34".parse().expect("an address"),
            literal: false,
        };
        let config = transport.curl_config(&url, Path::new("/tmp/fetch.headers"), Some(&pinned));
        assert!(
            config.contains("resolve = \"docs.example.com:443:93.184.216.34\""),
            "curl must connect to the address the guard checked: {config}"
        );

        let literal = PinnedTarget {
            host: "93.184.216.34".to_string(),
            port: 443,
            address: "93.184.216.34".parse().expect("an address"),
            literal: true,
        };
        let config = transport.curl_config(&url, Path::new("/tmp/fetch.headers"), Some(&literal));
        assert!(!config.contains("resolve ="), "{config}");
    }

    #[test]
    fn an_ipv6_pin_is_bracketed_for_curl() {
        let pinned = PinnedTarget {
            host: "docs.example.com".to_string(),
            port: 443,
            address: "2606:2800:220:1:248:1893:25c8:1946"
                .parse()
                .expect("an address"),
            literal: false,
        };
        assert_eq!(
            pinned.resolve_value().as_deref(),
            Some("docs.example.com:443:[2606:2800:220:1:248:1893:25c8:1946]")
        );
    }
}
