//! Spec 25, Task 4: the language-server protocol, without a process.
//!
//! LSP frames every message with a `Content-Length:` header and a blank line,
//! and a body may contain newlines, so the MCP transport's line framing does
//! not apply (`context.md` §2). Everything here runs over generic
//! `Read`/`Write`: [`read_message`] and [`write_message`] are the framing,
//! [`LspClient`] is the JSON-RPC layer (request ids, matching responses,
//! routing notifications, answering the server's own requests), and the
//! mapping functions turn what a server says into [`Symbol`]s, [`SymbolRef`]s
//! and [`FindingDraft`]s. Task 5 puts a child process's pipes under it.
//!
//! Only the requests proposal §5.4 names are implemented: the lifecycle
//! (`initialize`, `initialized`, `shutdown`, `exit`), `didOpen`/`didClose`,
//! `documentSymbol`, `definition`, `references`, `implementation`, and the
//! `publishDiagnostics` notification.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{ClientError, Result};
use crate::finding::{FindingDraft, FindingSource, Severity, SourceRange};
use crate::language::SymbolKind;
use crate::secret_scanner::SecretScanner;
use crate::symbol_index::{Provenance, Symbol, SymbolRef};

/// The largest body [`read_message`] accepts. A `documentSymbol` response for
/// a very large file is well under this; a claim above it is refused before
/// anything is read or allocated for it.
pub const MAX_LSP_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// One header line, including its `\r\n`. Real headers are a few dozen bytes;
/// the bound keeps a server that never sends a newline from growing a buffer.
const MAX_LSP_HEADER_LINE_BYTES: usize = 8 * 1024;

/// Header lines per message. LSP defines two.
const MAX_LSP_HEADER_LINES: usize = 32;

/// A body is read in steps of at most this much, so a `Content-Length` that
/// lies about a body that never comes costs no more than the bytes that did.
const INITIAL_BODY_CAPACITY: usize = 64 * 1024;

/// JSON-RPC's "method not found", the reply to a server request this client
/// does not implement. A reply of any kind keeps the server from waiting.
const METHOD_NOT_FOUND: i64 = -32601;

/// Writes one message: `Content-Length: N\r\n\r\n` and the body, where `N` is
/// the body's length in bytes, not characters.
pub fn write_message(writer: &mut impl Write, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message).map_err(|error| {
        ClientError::InvalidInput(format!(
            "Failed to encode a language server message: {error}"
        ))
    })?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .and_then(|()| writer.write_all(&body))
        .and_then(|()| writer.flush())
        .map_err(|error| ClientError::Io(format!("Failed to write to language server: {error}")))
}

/// Reads one message. `Ok(None)` is a clean end of stream before a message
/// began, which is how a server that exited looks. A missing or non-numeric
/// `Content-Length`, a body over [`MAX_LSP_MESSAGE_BYTES`], a stream that ends
/// inside a message, or a body that is not JSON is an error.
pub fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let Some(length) = read_headers(reader)? else {
        return Ok(None);
    };
    if length > MAX_LSP_MESSAGE_BYTES {
        return Err(ClientError::InvalidInput(format!(
            "Language server message of {length} bytes exceeds the {MAX_LSP_MESSAGE_BYTES}-byte limit"
        )));
    }
    let mut body = Vec::with_capacity(length.min(INITIAL_BODY_CAPACITY));
    reader
        .take(length as u64)
        .read_to_end(&mut body)
        .map_err(|error| {
            ClientError::Io(format!("Failed to read from language server: {error}"))
        })?;
    if body.len() != length {
        return Err(ClientError::Io(format!(
            "Language server stream ended mid-body: {} of {length} bytes",
            body.len()
        )));
    }
    serde_json::from_slice(&body).map(Some).map_err(|error| {
        ClientError::InvalidInput(format!(
            "Language server sent a body that is not JSON: {error}"
        ))
    })
}

/// Reads header lines up to the blank line and returns `Content-Length`.
/// Other headers (`Content-Type`) are ignored.
fn read_headers(reader: &mut impl BufRead) -> Result<Option<usize>> {
    let mut length = None;
    for index in 0..MAX_LSP_HEADER_LINES {
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_LSP_HEADER_LINE_BYTES as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| {
                ClientError::Io(format!("Failed to read from language server: {error}"))
            })?;
        if read == 0 {
            if index == 0 {
                return Ok(None);
            }
            return Err(ClientError::Io(
                "Language server stream ended mid-header".to_string(),
            ));
        }
        if line.last() != Some(&b'\n') {
            return Err(if line.len() >= MAX_LSP_HEADER_LINE_BYTES {
                ClientError::InvalidInput(format!(
                    "Language server header line exceeds {MAX_LSP_HEADER_LINE_BYTES} bytes"
                ))
            } else {
                ClientError::Io("Language server stream ended mid-header".to_string())
            });
        }
        let text = std::str::from_utf8(&line).map_err(|_| {
            ClientError::InvalidInput("Language server header is not UTF-8".to_string())
        })?;
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            return length.map(Some).ok_or_else(|| {
                ClientError::InvalidInput(
                    "Language server message has no Content-Length header".to_string(),
                )
            });
        }
        let Some((name, value)) = text.split_once(':') else {
            return Err(ClientError::InvalidInput(format!(
                "Language server sent a malformed header: {text:?}"
            )));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            let parsed = value.parse::<usize>().map_err(|_| {
                ClientError::InvalidInput(format!(
                    "Language server sent a non-numeric Content-Length: {value:?}"
                ))
            })?;
            if length.replace(parsed).is_some() {
                return Err(ClientError::InvalidInput(
                    "Language server message has two Content-Length headers".to_string(),
                ));
            }
        }
    }
    Err(ClientError::InvalidInput(format!(
        "Language server message has more than {MAX_LSP_HEADER_LINES} header lines"
    )))
}

/// The requests this client sends. A closed set: nothing outside proposal
/// §5.4 is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspRequest {
    Initialize,
    Shutdown,
    DocumentSymbol,
    Definition,
    References,
    Implementation,
}

impl LspRequest {
    pub fn method(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Shutdown => "shutdown",
            Self::DocumentSymbol => "textDocument/documentSymbol",
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Implementation => "textDocument/implementation",
        }
    }
}

/// A position in LSP's own terms: 0-based line, and 0-based character in
/// UTF-16 code units (the default encoding, which this client does not
/// renegotiate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    /// From Damaian's 1-based line and column, as `Symbol` stores them.
    pub fn from_one_based(line: u32, column: u32) -> Self {
        Self {
            line: line.saturating_sub(1),
            character: column.saturating_sub(1),
        }
    }

    fn to_json(self) -> Value {
        json!({ "line": self.line, "character": self.character })
    }
}

/// What one incoming message turned out to be, after [`LspClient::receive`]
/// has done whatever the protocol required of it.
#[derive(Debug)]
pub enum LspEvent {
    /// The response to a request this client sent. An error response is an
    /// `Err` carrying the server's message.
    Response {
        id: u64,
        request: LspRequest,
        outcome: Result<Value>,
    },
    /// `textDocument/publishDiagnostics`. Its `params` map through
    /// [`findings_from_diagnostics`]; each one replaces that file's set.
    Diagnostics(Value),
    /// Any other notification (`window/logMessage`, `$/progress`, …).
    Notification { method: String, params: Value },
    /// A request from the server, already answered.
    ServerRequest { method: String },
    /// A response whose id matches no pending request: one already given up
    /// on, or a server's mistake.
    Unmatched { id: Value },
}

/// The JSON-RPC side of a language-server connection. It owns the writer and
/// the table of requests awaiting a response; reading is the caller's, so
/// Task 5 can read on a thread and hand each message to [`Self::receive`] on
/// the thread that owns the client.
pub struct LspClient<W: Write> {
    writer: W,
    next_id: u64,
    pending: BTreeMap<u64, LspRequest>,
}

impl<W: Write> LspClient<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            next_id: 1,
            pending: BTreeMap::new(),
        }
    }

    /// `initialize`, with capabilities limited to what is used. `symbolKind`
    /// lists every kind, so a server reports `Struct` rather than falling
    /// back to the protocol's original 18 kinds.
    pub fn initialize(&mut self, uris: &RepositoryUris, process_id: u32) -> Result<u64> {
        let root_uri = uris.root_uri();
        let name = uris
            .root()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let params = json!({
            "processId": process_id,
            "clientInfo": { "name": "damaian", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": name }],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false },
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true,
                        "symbolKind": { "valueSet": (1..=26).collect::<Vec<u32>>() },
                    },
                    "definition": { "dynamicRegistration": false, "linkSupport": true },
                    "references": { "dynamicRegistration": false },
                    "implementation": { "dynamicRegistration": false, "linkSupport": true },
                    "publishDiagnostics": { "relatedInformation": false },
                },
            },
            "trace": "off",
        });
        self.request(LspRequest::Initialize, params)
    }

    pub fn initialized(&mut self) -> Result<()> {
        self.notify("initialized", json!({}))
    }

    pub fn shutdown(&mut self) -> Result<u64> {
        self.request(LspRequest::Shutdown, Value::Null)
    }

    pub fn exit(&mut self) -> Result<()> {
        self.notify("exit", Value::Null)
    }

    pub fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        )
    }

    pub fn did_close(&mut self, uri: &str) -> Result<()> {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    pub fn document_symbol(&mut self, uri: &str) -> Result<u64> {
        self.request(
            LspRequest::DocumentSymbol,
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    pub fn definition(&mut self, uri: &str, position: Position) -> Result<u64> {
        self.request(LspRequest::Definition, position_params(uri, position))
    }

    pub fn references(
        &mut self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<u64> {
        let mut params = position_params(uri, position);
        params["context"] = json!({ "includeDeclaration": include_declaration });
        self.request(LspRequest::References, params)
    }

    pub fn implementation(&mut self, uri: &str, position: Position) -> Result<u64> {
        self.request(LspRequest::Implementation, position_params(uri, position))
    }

    /// Stops waiting for a request, after a timeout. A late response to it is
    /// then [`LspEvent::Unmatched`].
    pub fn forget(&mut self, id: u64) -> Option<LspRequest> {
        self.pending.remove(&id)
    }

    /// How many requests await a response.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Classifies one incoming message and does what the protocol requires:
    /// a response is matched to its request, a server request is answered at
    /// once, a notification is routed. An error is a message that is not
    /// JSON-RPC at all, or a failure to write a reply.
    pub fn receive(&mut self, message: Value) -> Result<LspEvent> {
        let Value::Object(mut object) = message else {
            return Err(ClientError::InvalidInput(
                "Language server sent a message that is not a JSON object".to_string(),
            ));
        };
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let params = object.remove("params").unwrap_or(Value::Null);
        match (method, object.remove("id")) {
            (Some(method), Some(id)) => {
                self.answer(id, &method, &params)?;
                Ok(LspEvent::ServerRequest { method })
            }
            (Some(method), None) if method == "textDocument/publishDiagnostics" => {
                Ok(LspEvent::Diagnostics(params))
            }
            (Some(method), None) => Ok(LspEvent::Notification { method, params }),
            (None, Some(id)) if object.contains_key("result") || object.contains_key("error") => {
                let Some(request) = id.as_u64().and_then(|key| self.pending.remove(&key)) else {
                    return Ok(LspEvent::Unmatched { id });
                };
                let outcome = match object.remove("error") {
                    Some(error) => Err(response_error(request, &error)),
                    None => Ok(object.remove("result").unwrap_or(Value::Null)),
                };
                Ok(LspEvent::Response {
                    id: id.as_u64().unwrap_or_default(),
                    request,
                    outcome,
                })
            }
            _ => Err(ClientError::InvalidInput(
                "Language server sent a message that is neither a request, a response nor a notification"
                    .to_string(),
            )),
        }
    }

    /// Feeds messages from `next` through [`Self::receive`] until the response
    /// to `id` arrives, handing every other event to `on_event`. `next`
    /// returning `None` means the server is gone. Task 5 bounds the wait by
    /// making `next` a `recv_timeout` on its reader thread's channel.
    pub fn await_response(
        &mut self,
        id: u64,
        mut next: impl FnMut() -> Result<Option<Value>>,
        mut on_event: impl FnMut(LspEvent),
    ) -> Result<Value> {
        loop {
            let Some(message) = next()? else {
                self.pending.remove(&id);
                return Err(ClientError::Io(
                    "Language server closed its output before responding".to_string(),
                ));
            };
            match self.receive(message)? {
                LspEvent::Response {
                    id: answered,
                    outcome,
                    ..
                } if answered == id => return outcome,
                event => on_event(event),
            }
        }
    }

    fn request(&mut self, request: LspRequest, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": request.method() });
        if !params.is_null() {
            message["params"] = params;
        }
        write_message(&mut self.writer, &message)?;
        self.pending.insert(id, request);
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        write_message(&mut self.writer, &message)
    }

    /// The minimal valid reply to a server request, so a server never stalls
    /// waiting for one. `workspace/configuration` gets one `null` per item
    /// asked for, which means "use your defaults".
    fn answer(&mut self, id: Value, method: &str, params: &Value) -> Result<()> {
        let reply = match method {
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                json!({ "jsonrpc": "2.0", "id": id, "result": vec![Value::Null; items] })
            }
            "client/registerCapability" | "window/workDoneProgress/create" => {
                json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null })
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": METHOD_NOT_FOUND, "message": format!("{method} is not supported") },
            }),
        };
        write_message(&mut self.writer, &reply)
    }
}

fn position_params(uri: &str, position: Position) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": position.to_json() })
}

fn response_error(request: LspRequest, error: &Value) -> ClientError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no message");
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    ClientError::Io(format!(
        "Language server refused {}: {message} (code {code})",
        request.method()
    ))
}

/// Converts between repository-relative paths and the `file://` URIs a
/// server speaks. `root` must be the path the server was initialised with,
/// in the same form: no canonicalisation happens here.
#[derive(Debug, Clone)]
pub struct RepositoryUris {
    root: PathBuf,
}

impl RepositoryUris {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn root_uri(&self) -> String {
        format!(
            "file://{}",
            percent_encode_path(&self.root.to_string_lossy())
        )
    }

    /// The URI of a repository-relative path.
    pub fn uri_for(&self, relative: &str) -> String {
        let path = self.root.join(relative);
        format!("file://{}", percent_encode_path(&path.to_string_lossy()))
    }

    /// The repository-relative path a URI names, or `None` for anything that
    /// is not a `file://` URI inside the root: another scheme, a remote host,
    /// a malformed escape, a `..` segment, a sibling such as `/repo-two` for
    /// root `/repo`, or the root itself.
    pub fn relative_path(&self, uri: &str) -> Option<String> {
        let rest = uri.strip_prefix("file://")?;
        let encoded = match rest.strip_prefix("localhost") {
            Some(path) if path.starts_with('/') => path,
            _ if rest.starts_with('/') => rest,
            _ => return None,
        };
        if encoded.contains(['?', '#']) {
            return None;
        }
        let decoded = percent_decode(encoded)?;
        let path = Path::new(&decoded);
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return None;
        }
        let relative = path.strip_prefix(&self.root).ok()?;
        if relative.as_os_str().is_empty() {
            return None;
        }
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

/// Escapes everything but RFC 3986's unreserved characters and `/`.
fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `None` for a truncated or non-hex escape, or bytes that are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            decoded.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// LSP's numeric `SymbolKind` to proposal §5.1's, per `context.md` §8. `None`
/// for kinds that are not indexed: fields, properties, enum members, type
/// parameters, literals and the rest multiply the index without answering
/// "where is X defined". LSP has no `Trait`, so rust-analyzer's traits arrive
/// as `Interface` and stay that.
pub fn symbol_kind_from_lsp(kind: u64) -> Option<SymbolKind> {
    Some(match kind {
        2..=4 => SymbolKind::Module,
        5 => SymbolKind::Class,
        6 | 9 => SymbolKind::Method,
        10 => SymbolKind::Enum,
        11 => SymbolKind::Interface,
        12 => SymbolKind::Function,
        13 => SymbolKind::Variable,
        14 => SymbolKind::Constant,
        23 => SymbolKind::Struct,
        _ => return None,
    })
}

/// A `textDocument/documentSymbol` result for the file at `path` (repository
/// relative, in spec 24 root `root_path`) as `Symbol`s, sorted by line,
/// column, name and container. Both answer shapes are accepted:
///
/// - hierarchical `DocumentSymbol`: the line and column are the start of
///   `selectionRange` (the name), not of `range`, which starts at a doc
///   comment or attribute the heuristic tier never sees. A child's container
///   is its parent's name, whatever the parent's kind; a parent of a kind
///   that is not indexed (rust-analyzer's `impl Foo` object) still names it.
/// - flat `SymbolInformation`: the line is `location.range`'s start, the only
///   range it has, and the container is `containerName`. An entry whose URI
///   is not `path` is dropped.
///
/// Lines and columns are converted from 0-based to 1-based. A symbol whose
/// name or container the scanner flags is dropped, as the heuristic tier
/// drops it (`context.md` §4.2).
pub fn symbols_from_document_symbols(
    result: &Value,
    uris: &RepositoryUris,
    path: &str,
    root_path: &str,
    scanner: &SecretScanner,
) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let context = SymbolContext {
        uris,
        path,
        root_path,
        scanner,
    };
    for item in result.as_array().into_iter().flatten() {
        if item.get("selectionRange").is_some() {
            context.collect_document_symbol(item, None, &mut symbols);
        } else if item.get("location").is_some() {
            context.collect_symbol_information(item, &mut symbols);
        }
    }
    symbols.sort_by(|left, right| {
        (left.line, left.column, &left.name, &left.container).cmp(&(
            right.line,
            right.column,
            &right.name,
            &right.container,
        ))
    });
    symbols
}

struct SymbolContext<'a> {
    uris: &'a RepositoryUris,
    path: &'a str,
    root_path: &'a str,
    scanner: &'a SecretScanner,
}

impl SymbolContext<'_> {
    fn collect_document_symbol(
        &self,
        item: &Value,
        container: Option<&str>,
        symbols: &mut Vec<Symbol>,
    ) {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            return;
        };
        if let Some(kind) = item
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(symbol_kind_from_lsp)
            && let Some((line, column)) = range_start(item.get("selectionRange"))
        {
            self.push(name, kind, line, column, container, symbols);
        }
        for child in item
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.collect_document_symbol(child, Some(name), symbols);
        }
    }

    fn collect_symbol_information(&self, item: &Value, symbols: &mut Vec<Symbol>) {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            return;
        };
        let Some(kind) = item
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(symbol_kind_from_lsp)
        else {
            return;
        };
        let Some(location) = item.get("location") else {
            return;
        };
        let uri = location
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if self.uris.relative_path(uri).as_deref() != Some(self.path) {
            return;
        }
        let Some((line, column)) = range_start(location.get("range")) else {
            return;
        };
        let container = item
            .get("containerName")
            .and_then(Value::as_str)
            .filter(|container| !container.is_empty());
        self.push(name, kind, line, column, container, symbols);
    }

    fn push(
        &self,
        name: &str,
        kind: SymbolKind,
        line: u32,
        column: u32,
        container: Option<&str>,
        symbols: &mut Vec<Symbol>,
    ) {
        if self.scanner.contains_secrets(name)
            || container.is_some_and(|container| self.scanner.contains_secrets(container))
        {
            return;
        }
        symbols.push(Symbol {
            name: name.to_string(),
            kind,
            path: self.path.to_string(),
            line,
            column: Some(column),
            container: container.map(str::to_string),
            root_path: self.root_path.to_string(),
            provenance: Provenance::LanguageServer,
        });
    }
}

/// A range's start as 1-based `(line, column)`.
fn range_start(range: Option<&Value>) -> Option<(u32, u32)> {
    position_one_based(range?.get("start")?)
}

fn range_end(range: Option<&Value>) -> Option<(u32, u32)> {
    position_one_based(range?.get("end")?)
}

fn position_one_based(position: &Value) -> Option<(u32, u32)> {
    let line = u32::try_from(position.get("line")?.as_u64()?).ok()?;
    let character = u32::try_from(position.get("character")?.as_u64()?).ok()?;
    Some((line.checked_add(1)?, character.checked_add(1)?))
}

/// A `definition`, `references` or `implementation` result as `SymbolRef`s:
/// `null`, one `Location`, a `Location[]` or a `LocationLink[]` (whose
/// `targetSelectionRange` is the name). A location outside the repository is
/// dropped. A location has no name, so `name` is empty, as for the
/// file-level ends `SymbolIndex` already writes; the caller names it.
pub fn symbol_refs_from_locations(result: &Value, uris: &RepositoryUris) -> Vec<SymbolRef> {
    let items: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (uri, item.get("targetSelectionRange")),
                None => (item.get("uri")?, item.get("range")),
            };
            let path = uris.relative_path(uri.as_str()?)?;
            let (line, _) = range_start(range)?;
            Some(SymbolRef {
                path,
                name: String::new(),
                line,
            })
        })
        .collect()
}

/// One `publishDiagnostics` notification's findings. It replaces every
/// earlier set for `path`, so an empty `findings` clears the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub path: String,
    pub findings: Vec<FindingDraft>,
}

/// `publishDiagnostics` params as findings (`context.md` §10), or `None` when
/// the URI is outside the repository. Severity 1 is `Error`, 2 `Warning`, 3
/// and 4 `Info`; an absent severity is `Error`, as editors read it. Ranges
/// become 1-based. A message's first line is the summary and the whole
/// message the details when it has more than one; `Finding::new` redacts and
/// bounds both. Restricted files are the caller's to drop: this layer knows
/// nothing of the path policy.
pub fn findings_from_diagnostics(params: &Value, uris: &RepositoryUris) -> Option<FileDiagnostics> {
    let path = uris.relative_path(params.get("uri")?.as_str()?)?;
    let findings = params
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|diagnostic| diagnostic_draft(diagnostic, &path))
        .collect();
    Some(FileDiagnostics { path, findings })
}

fn diagnostic_draft(diagnostic: &Value, path: &str) -> FindingDraft {
    let severity = match diagnostic.get("severity").and_then(Value::as_u64) {
        None | Some(1) => Severity::Error,
        Some(2) => Severity::Warning,
        Some(_) => Severity::Info,
    };
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let details = message.contains('\n').then(|| message.clone());
    let range = diagnostic.get("range");
    let source_range = range_start(range).map(|(start_line, start_column)| {
        let end = range_end(range);
        SourceRange {
            path: path.to_string(),
            start_line,
            start_column: Some(start_column),
            end_line: end.map(|(line, _)| line),
            end_column: end.map(|(_, column)| column),
        }
    });
    let code = match diagnostic.get("code") {
        Some(Value::String(code)) => Some(code.clone()),
        Some(Value::Number(code)) => Some(code.to_string()),
        _ => None,
    };
    FindingDraft {
        source: FindingSource::LanguageServer,
        severity,
        summary: message,
        details,
        range: source_range,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(body);
        framed
    }

    fn read_error(input: &[u8]) -> String {
        match read_message(&mut Cursor::new(input)) {
            Err(error) => error.to_string(),
            Ok(message) => panic!("expected an error, read {message:?}"),
        }
    }

    fn frames(messages: &[Value]) -> Cursor<Vec<u8>> {
        let mut buffer = Vec::new();
        for message in messages {
            write_message(&mut buffer, message).unwrap();
        }
        Cursor::new(buffer)
    }

    /// Every message the client wrote, in order.
    fn written(client: &LspClient<Vec<u8>>) -> Vec<Value> {
        let mut reader = Cursor::new(client.writer().clone());
        let mut messages = Vec::new();
        while let Some(message) = read_message(&mut reader).unwrap() {
            messages.push(message);
        }
        messages
    }

    fn uris() -> RepositoryUris {
        RepositoryUris::new("/work/repo")
    }

    #[test]
    fn a_message_round_trips_with_its_length_in_bytes() {
        let message = json!({ "text": "line one\r\nline two ✓ ünï 🎉", "n": 1 });
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).unwrap();
        let body = serde_json::to_vec(&message).unwrap();
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        assert!(body.len() > String::from_utf8(body.clone()).unwrap().chars().count());
        assert!(buffer.starts_with(header.as_bytes()));
        assert_eq!(buffer.len(), header.len() + body.len());

        let mut reader = Cursor::new(buffer);
        assert_eq!(read_message(&mut reader).unwrap(), Some(message));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn a_body_with_raw_crlf_and_multibyte_text_is_read_exactly() {
        // JSON whitespace may hold a raw blank line, which must not be taken
        // for the end of a header block, and the next message must start
        // exactly where the length says.
        let first = "{\r\n\r\n  \"name\": \"Grüße ✓ 🎉\"\r\n}\r\n";
        let mut input = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{first}",
            first.len()
        )
        .into_bytes();
        input.extend(frame(br#"{"second":true}"#));
        let mut reader = Cursor::new(input);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({ "name": "Grüße ✓ 🎉" }))
        );
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({ "second": true }))
        );
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn a_header_name_is_case_insensitive_and_other_headers_are_ignored() {
        let input = b"X-Other: 1\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(
            read_message(&mut Cursor::new(&input[..])).unwrap(),
            Some(json!({}))
        );
    }

    #[test]
    fn each_malformed_frame_is_an_error() {
        assert!(
            read_error(b"Content-Type: application/json\r\n\r\n{}").contains("no Content-Length")
        );
        assert!(read_error(b"Content-Length: twelve\r\n\r\n{}").contains("non-numeric"));
        assert!(read_error(b"Content-Length: -2\r\n\r\n{}").contains("non-numeric"));
        assert!(
            read_error(b"Content-Length: 99999999999999999999999999\r\n\r\n")
                .contains("non-numeric")
        );
        assert!(
            read_error(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}")
                .contains("two Content-Length")
        );
        assert!(read_error(b"Content-Length 2\r\n\r\n{}").contains("malformed header"));
        assert!(read_error(b"Content-Length: 10\r\n\r\n{\"a\":").contains("mid-body"));
        assert!(read_error(b"Content-Length: 2\r\n").contains("mid-header"));
        assert!(read_error(b"Content-Len").contains("mid-header"));
        assert!(read_error(b"Content-Length: 3\r\n\r\nnot").contains("not JSON"));
        let many = "X: 1\r\n".repeat(MAX_LSP_HEADER_LINES + 1);
        assert!(read_error(many.as_bytes()).contains("header lines"));
    }

    #[test]
    fn a_header_line_without_a_newline_is_cut_at_its_bound() {
        // The error text alone would not show the bound: an unbounded read
        // of this input fails the same way. Where the reader stopped does.
        let mut reader = Cursor::new(vec![b'x'; MAX_LSP_HEADER_LINE_BYTES * 4]);
        let error = read_message(&mut reader).unwrap_err().to_string();
        assert!(error.contains("header line exceeds"), "{error}");
        assert_eq!(reader.position(), MAX_LSP_HEADER_LINE_BYTES as u64);
    }

    #[test]
    fn an_oversized_length_is_refused_before_the_body_is_read() {
        // A claim one byte over the limit, followed by fewer bytes than it
        // claims: without the bound this is a mid-body error instead, and
        // the reader has consumed the body. A claim of `usize::MAX` would be
        // an allocation failure if the claimed size were allocated first.
        let over = MAX_LSP_MESSAGE_BYTES + 1;
        let header = format!("Content-Length: {over}\r\n\r\n");
        let mut input = header.clone().into_bytes();
        input.extend_from_slice(b"{\"partial\":");
        let mut reader = Cursor::new(input);
        let error = read_message(&mut reader).unwrap_err().to_string();
        assert!(error.contains("exceeds the 16777216-byte limit"), "{error}");
        assert_eq!(reader.position(), header.len() as u64);
        let error = read_error(format!("Content-Length: {}\r\n\r\n", usize::MAX).as_bytes());
        assert!(error.contains("exceeds"), "{error}");
    }

    #[test]
    fn a_body_of_exactly_the_limit_is_read() {
        let mut body = vec![b'"'; 1];
        body.resize(MAX_LSP_MESSAGE_BYTES - 1, b'a');
        body.push(b'"');
        let message = read_message(&mut Cursor::new(frame(&body)))
            .unwrap()
            .unwrap();
        assert_eq!(
            message.as_str().map(str::len),
            Some(MAX_LSP_MESSAGE_BYTES - 2)
        );
    }

    #[test]
    fn requests_carry_increasing_ids_and_the_methods_named() {
        let mut client = LspClient::new(Vec::new());
        assert_eq!(client.initialize(&uris(), 42).unwrap(), 1);
        client.initialized().unwrap();
        client
            .did_open("file:///work/repo/a.rs", "rust", 1, "fn a() {}\n")
            .unwrap();
        assert_eq!(client.document_symbol("file:///work/repo/a.rs").unwrap(), 2);
        let at = Position::from_one_based(3, 5);
        assert_eq!(client.definition("file:///work/repo/a.rs", at).unwrap(), 3);
        assert_eq!(
            client
                .references("file:///work/repo/a.rs", at, false)
                .unwrap(),
            4
        );
        assert_eq!(
            client.implementation("file:///work/repo/a.rs", at).unwrap(),
            5
        );
        client.did_close("file:///work/repo/a.rs").unwrap();
        assert_eq!(client.shutdown().unwrap(), 6);
        client.exit().unwrap();
        assert_eq!(client.pending(), 6);

        let messages = written(&client);
        let methods: Vec<&str> = messages
            .iter()
            .map(|message| message["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "textDocument/didOpen",
                "textDocument/documentSymbol",
                "textDocument/definition",
                "textDocument/references",
                "textDocument/implementation",
                "textDocument/didClose",
                "shutdown",
                "exit",
            ]
        );
        let initialize = &messages[0]["params"];
        assert_eq!(initialize["processId"], 42);
        assert_eq!(initialize["rootUri"], "file:///work/repo");
        assert_eq!(
            initialize["capabilities"]["textDocument"]["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
            true
        );
        assert!(messages[1].get("id").is_none());
        assert_eq!(messages[2]["params"]["textDocument"]["languageId"], "rust");
        assert_eq!(
            messages[4]["params"]["position"],
            json!({ "line": 2, "character": 4 })
        );
        assert_eq!(
            messages[5]["params"]["context"]["includeDeclaration"],
            false
        );
        assert!(messages[8].get("params").is_none());
    }

    #[test]
    fn a_response_after_an_unrelated_notification_still_matches_its_request() {
        let mut client = LspClient::new(Vec::new());
        let symbols = client.document_symbol("file:///work/repo/a.rs").unwrap();
        let definition = client
            .definition("file:///work/repo/a.rs", Position::from_one_based(1, 1))
            .unwrap();
        let mut incoming = frames(&[
            json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 3, "message": "indexing" } }),
            json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "uri": "file:///work/repo/a.rs", "diagnostics": [] } }),
            json!({ "jsonrpc": "2.0", "id": definition, "result": [] }),
            json!({ "jsonrpc": "2.0", "id": 99, "result": null }),
            json!({ "jsonrpc": "2.0", "id": symbols, "result": [{ "name": "a" }] }),
        ]);
        let mut events = Vec::new();
        let result = client
            .await_response(
                symbols,
                || read_message(&mut incoming),
                |event| events.push(event),
            )
            .unwrap();
        assert_eq!(result, json!([{ "name": "a" }]));
        assert_eq!(client.pending(), 0);

        assert_eq!(events.len(), 4, "{events:?}");
        assert!(matches!(
            &events[0],
            LspEvent::Notification { method, params }
                if method == "window/logMessage" && params["message"] == "indexing"
        ));
        assert!(
            matches!(&events[1], LspEvent::Diagnostics(params) if params["uri"] == "file:///work/repo/a.rs")
        );
        assert!(matches!(
            &events[2],
            LspEvent::Response { id, request: LspRequest::Definition, outcome: Ok(_) } if *id == definition
        ));
        assert!(matches!(&events[3], LspEvent::Unmatched { id } if *id == json!(99)));
    }

    #[test]
    fn an_error_response_becomes_a_client_error() {
        let mut client = LspClient::new(Vec::new());
        let id = client.document_symbol("file:///work/repo/a.rs").unwrap();
        let mut incoming = frames(&[json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32801, "message": "content modified" },
        })]);
        let error = client
            .await_response(id, || read_message(&mut incoming), |_| {})
            .unwrap_err();
        assert!(matches!(error, ClientError::Io(_)));
        let message = error.to_string();
        assert!(message.contains("textDocument/documentSymbol"), "{message}");
        assert!(message.contains("content modified"), "{message}");
        assert!(message.contains("-32801"), "{message}");
        assert_eq!(client.pending(), 0);
    }

    #[test]
    fn the_end_of_the_stream_before_a_response_is_an_error() {
        let mut client = LspClient::new(Vec::new());
        let id = client.shutdown().unwrap();
        let mut incoming = Cursor::new(Vec::new());
        let error = client
            .await_response(id, || read_message(&mut incoming), |_| {})
            .unwrap_err();
        assert!(error.to_string().contains("closed its output"));
        assert_eq!(client.pending(), 0);
    }

    #[test]
    fn a_forgotten_request_s_late_response_is_unmatched() {
        let mut client = LspClient::new(Vec::new());
        let id = client.document_symbol("file:///work/repo/a.rs").unwrap();
        assert_eq!(client.forget(id), Some(LspRequest::DocumentSymbol));
        let event = client
            .receive(json!({ "jsonrpc": "2.0", "id": id, "result": [] }))
            .unwrap();
        assert!(matches!(event, LspEvent::Unmatched { .. }));
    }

    #[test]
    fn server_requests_get_minimal_valid_replies() {
        let mut client = LspClient::new(Vec::new());
        let requests = [
            json!({ "jsonrpc": "2.0", "id": 7, "method": "workspace/configuration", "params": { "items": [{ "section": "a" }, { "section": "b" }] } }),
            json!({ "jsonrpc": "2.0", "id": "reg", "method": "client/registerCapability", "params": { "registrations": [] } }),
            json!({ "jsonrpc": "2.0", "id": 8, "method": "window/workDoneProgress/create", "params": { "token": "t" } }),
            json!({ "jsonrpc": "2.0", "id": 9, "method": "window/showDocument", "params": {} }),
        ];
        for request in requests {
            let event = client.receive(request.clone()).unwrap();
            assert!(
                matches!(&event, LspEvent::ServerRequest { method } if *method == request["method"])
            );
        }
        let replies = written(&client);
        assert_eq!(
            replies,
            [
                json!({ "jsonrpc": "2.0", "id": 7, "result": [null, null] }),
                json!({ "jsonrpc": "2.0", "id": "reg", "result": null }),
                json!({ "jsonrpc": "2.0", "id": 8, "result": null }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 9,
                    "error": { "code": -32601, "message": "window/showDocument is not supported" },
                }),
            ]
        );
    }

    #[test]
    fn a_message_that_is_not_json_rpc_is_an_error() {
        let mut client = LspClient::new(Vec::new());
        assert!(client.receive(json!([1, 2])).is_err());
        assert!(client.receive(json!({ "jsonrpc": "2.0" })).is_err());
        assert!(
            client
                .receive(json!({ "jsonrpc": "2.0", "id": 1 }))
                .is_err()
        );
    }

    fn position(line: u32, character: u32) -> Value {
        json!({ "line": line, "character": character })
    }

    fn range(start: (u32, u32), end: (u32, u32)) -> Value {
        json!({ "start": position(start.0, start.1), "end": position(end.0, end.1) })
    }

    /// A struct whose doc comment starts `range` four lines above its name,
    /// a method under it, an `impl` block (an object, not indexed) holding a
    /// constructor, an excluded field and type parameter, and a free function.
    fn hierarchical() -> Value {
        json!([
            {
                "name": "Uploader", "kind": 23,
                "range": range((6, 0), (20, 1)),
                "selectionRange": range((10, 11), (10, 19)),
                "children": [
                    { "name": "attempts", "kind": 8, "range": range((11, 4), (11, 20)), "selectionRange": range((11, 4), (11, 12)) },
                ],
            },
            {
                "name": "impl Uploader", "kind": 19,
                "range": range((21, 0), (40, 1)),
                "selectionRange": range((21, 5), (21, 13)),
                "children": [
                    { "name": "new", "kind": 9, "range": range((22, 4), (24, 5)), "selectionRange": range((22, 11), (22, 14)) },
                    { "name": "upload_retry", "kind": 6, "range": range((25, 4), (39, 5)), "selectionRange": range((28, 7), (28, 19)) },
                ],
            },
            { "name": "T", "kind": 26, "range": range((41, 0), (41, 1)), "selectionRange": range((41, 0), (41, 1)) },
            { "name": "helper", "kind": 12, "range": range((42, 0), (45, 1)), "selectionRange": range((43, 3), (43, 9)) },
        ])
    }

    /// The same symbols, flat. `SymbolInformation` has no `selectionRange`,
    /// so its `range` starts at the name.
    fn flat() -> Value {
        let at = |line: u32, character: u32| json!({ "uri": "file:///work/repo/src/upload.rs", "range": range((line, character), (line, character + 3)) });
        json!([
            { "name": "helper", "kind": 12, "location": at(43, 3) },
            { "name": "upload_retry", "kind": 6, "location": at(28, 7), "containerName": "impl Uploader" },
            { "name": "Uploader", "kind": 23, "location": at(10, 11), "containerName": "" },
            { "name": "attempts", "kind": 8, "location": at(11, 4), "containerName": "Uploader" },
            { "name": "new", "kind": 9, "location": at(22, 11), "containerName": "impl Uploader" },
            { "name": "T", "kind": 26, "location": at(41, 0) },
            { "name": "elsewhere", "kind": 12, "location": { "uri": "file:///work/repo/src/other.rs", "range": range((1, 0), (1, 3)) } },
        ])
    }

    fn symbols_for(result: &Value) -> Vec<Symbol> {
        symbols_from_document_symbols(
            result,
            &uris(),
            "src/upload.rs",
            "",
            &SecretScanner::default(),
        )
    }

    fn symbol(
        name: &str,
        kind: SymbolKind,
        line: u32,
        column: u32,
        container: Option<&str>,
    ) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind,
            path: "src/upload.rs".to_string(),
            line,
            column: Some(column),
            container: container.map(str::to_string),
            root_path: String::new(),
            provenance: Provenance::LanguageServer,
        }
    }

    #[test]
    fn hierarchical_and_flat_document_symbols_give_the_same_symbols() {
        let expected = vec![
            symbol("Uploader", SymbolKind::Struct, 11, 12, None),
            symbol("new", SymbolKind::Method, 23, 12, Some("impl Uploader")),
            symbol(
                "upload_retry",
                SymbolKind::Method,
                29,
                8,
                Some("impl Uploader"),
            ),
            symbol("helper", SymbolKind::Function, 44, 4, None),
        ];
        assert_eq!(symbols_for(&hierarchical()), expected);
        assert_eq!(symbols_for(&flat()), expected);
    }

    #[test]
    fn a_document_symbol_s_line_is_its_name_not_its_doc_comment() {
        // `range` starts at line 7 (1-based), the doc comment; the name is on
        // line 11. Pinned separately so it does not rest on the flat fixture.
        let symbols = symbols_for(&hierarchical());
        assert_eq!(symbols[0].name, "Uploader");
        assert_eq!((symbols[0].line, symbols[0].column), (11, Some(12)));
    }

    #[test]
    fn an_empty_or_null_document_symbol_result_gives_nothing() {
        assert!(symbols_for(&Value::Null).is_empty());
        assert!(symbols_for(&json!([])).is_empty());
    }

    #[test]
    fn a_secret_in_a_symbol_s_name_or_container_drops_it() {
        let scanner = SecretScanner::new(vec!["hunter2".to_string()]);
        let result = json!([
            { "name": "hunter2", "kind": 12, "range": range((0, 0), (0, 9)), "selectionRange": range((0, 3), (0, 9)) },
            {
                "name": "Holder_hunter2", "kind": 23,
                "range": range((2, 0), (5, 1)), "selectionRange": range((2, 7), (2, 13)),
                "children": [
                    { "name": "inner", "kind": 6, "range": range((3, 4), (3, 9)), "selectionRange": range((3, 4), (3, 9)) },
                ],
            },
            { "name": "kept", "kind": 12, "range": range((6, 0), (6, 9)), "selectionRange": range((6, 3), (6, 7)) },
        ]);
        let names: Vec<String> =
            symbols_from_document_symbols(&result, &uris(), "src/a.rs", "", &scanner)
                .into_iter()
                .map(|symbol| symbol.name)
                .collect();
        assert_eq!(names, ["kept"]);
    }

    #[test]
    fn lsp_symbol_kinds_map_per_the_table() {
        use SymbolKind::*;
        let expected: [(u64, Option<SymbolKind>); 27] = [
            (1, None),
            (2, Some(Module)),
            (3, Some(Module)),
            (4, Some(Module)),
            (5, Some(Class)),
            (6, Some(Method)),
            (7, None),
            (8, None),
            (9, Some(Method)),
            (10, Some(Enum)),
            (11, Some(Interface)),
            (12, Some(Function)),
            (13, Some(Variable)),
            (14, Some(Constant)),
            (15, None),
            (16, None),
            (17, None),
            (18, None),
            (19, None),
            (20, None),
            (21, None),
            (22, None),
            (23, Some(Struct)),
            (24, None),
            (25, None),
            (26, None),
            (27, None),
        ];
        for (kind, mapped) in expected {
            assert_eq!(symbol_kind_from_lsp(kind), mapped, "LSP kind {kind}");
        }
    }

    #[test]
    fn uris_inside_the_root_map_to_relative_paths_and_others_are_dropped() {
        let uris = uris();
        assert_eq!(
            uris.relative_path("file:///work/repo/src/main.rs")
                .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            uris.relative_path("file://localhost/work/repo/a.rs")
                .as_deref(),
            Some("a.rs")
        );
        assert_eq!(
            uris.relative_path("file:///work/repo/src/my%20file%C3%A9%2Brs.rs")
                .as_deref(),
            Some("src/my fileé+rs.rs")
        );
        // Lower-case hex is valid too.
        assert_eq!(
            uris.relative_path("file:///work/repo/%c3%a9.rs").as_deref(),
            Some("é.rs")
        );
        for outside in [
            "file:///etc/passwd",
            "file:///work/repo-two/a.rs",
            "file:///work/repo/../repo-two/a.rs",
            "file:///work/repo/src/%2E%2E/%2E%2E/x.rs",
            "file:///work/repo",
            "file:///work/repo/",
            "file://remote-host/work/repo/a.rs",
            "file:///work/repo/a%ZZ.rs",
            "file:///work/repo/a%C3.rs",
            "file:///work/repo/a%2",
            "file:///work/repo/a.rs?x=1",
            "untitled:Untitled-1",
            "https://example.com/work/repo/a.rs",
        ] {
            assert_eq!(uris.relative_path(outside), None, "{outside}");
        }
    }

    #[test]
    fn a_uri_built_for_a_path_maps_back_to_it() {
        let uris = RepositoryUris::new("/work/my repo");
        assert_eq!(uris.root_uri(), "file:///work/my%20repo");
        let uri = uris.uri_for("src/grüße #1.rs");
        assert_eq!(uri, "file:///work/my%20repo/src/gr%C3%BC%C3%9Fe%20%231.rs");
        assert_eq!(uris.relative_path(&uri).as_deref(), Some("src/grüße #1.rs"));
    }

    #[test]
    fn locations_and_location_links_become_symbol_refs() {
        let uris = uris();
        let reference = |path: &str, line: u32| SymbolRef {
            path: path.to_string(),
            name: String::new(),
            line,
        };
        let single = json!({ "uri": "file:///work/repo/src/a.rs", "range": range((4, 2), (4, 8)) });
        assert_eq!(
            symbol_refs_from_locations(&single, &uris),
            [reference("src/a.rs", 5)]
        );
        let many = json!([
            { "uri": "file:///work/repo/src/a.rs", "range": range((0, 0), (0, 1)) },
            { "uri": "file:///home/user/.cargo/registry/serde/lib.rs", "range": range((9, 0), (9, 1)) },
            { "uri": "file:///work/repo/src/b%20c.rs", "range": range((19, 0), (19, 1)) },
        ]);
        assert_eq!(
            symbol_refs_from_locations(&many, &uris),
            [reference("src/a.rs", 1), reference("src/b c.rs", 20)]
        );
        let links = json!([{
            "targetUri": "file:///work/repo/src/a.rs",
            "targetRange": range((2, 0), (9, 1)),
            "targetSelectionRange": range((6, 3), (6, 9)),
        }]);
        assert_eq!(
            symbol_refs_from_locations(&links, &uris),
            [reference("src/a.rs", 7)]
        );
        assert!(symbol_refs_from_locations(&Value::Null, &uris).is_empty());
    }

    #[test]
    fn each_diagnostic_severity_maps_and_ranges_become_one_based() {
        let diagnostic = |severity: Option<u64>, message: &str| {
            let mut diagnostic = json!({
                "range": range((9, 4), (9, 12)),
                "message": message,
            });
            if let Some(severity) = severity {
                diagnostic["severity"] = json!(severity);
            }
            diagnostic
        };
        let params = json!({
            "uri": "file:///work/repo/src/x.rs",
            "version": 3,
            "diagnostics": [
                diagnostic(Some(1), "mismatched types"),
                diagnostic(Some(2), "unused variable"),
                diagnostic(Some(3), "consider this"),
                diagnostic(Some(4), "hint"),
                diagnostic(None, "no severity"),
            ],
        });
        let file = findings_from_diagnostics(&params, &uris()).unwrap();
        assert_eq!(file.path, "src/x.rs");
        let severities: Vec<Severity> = file.findings.iter().map(|draft| draft.severity).collect();
        assert_eq!(
            severities,
            [
                Severity::Error,
                Severity::Warning,
                Severity::Info,
                Severity::Info,
                Severity::Error,
            ]
        );
        for draft in &file.findings {
            assert_eq!(draft.source, FindingSource::LanguageServer);
            assert_eq!(
                draft.range,
                Some(SourceRange {
                    path: "src/x.rs".to_string(),
                    start_line: 10,
                    start_column: Some(5),
                    end_line: Some(10),
                    end_column: Some(13),
                })
            );
        }
        assert_eq!(file.findings[0].summary, "mismatched types");
        assert_eq!(file.findings[0].details, None);
    }

    #[test]
    fn a_diagnostic_s_code_and_multi_line_message_are_kept() {
        let params = json!({
            "uri": "file:///work/repo/src/x.rs",
            "diagnostics": [
                { "range": range((0, 0), (0, 1)), "severity": 1, "code": "E0308", "message": "mismatched types\nexpected u32" },
                { "range": range((1, 0), (1, 1)), "severity": 2, "code": 6133, "message": "unused" },
                { "severity": 2, "message": "no range" },
            ],
        });
        let file = findings_from_diagnostics(&params, &uris()).unwrap();
        assert_eq!(file.findings[0].code.as_deref(), Some("E0308"));
        assert_eq!(
            file.findings[0].details.as_deref(),
            Some("mismatched types\nexpected u32")
        );
        assert_eq!(file.findings[1].code.as_deref(), Some("6133"));
        assert_eq!(file.findings[2].range, None);
    }

    #[test]
    fn diagnostics_outside_the_root_are_dropped_and_an_empty_set_clears_a_file() {
        let outside =
            json!({ "uri": "file:///elsewhere/x.rs", "diagnostics": [{ "message": "m" }] });
        assert_eq!(findings_from_diagnostics(&outside, &uris()), None);
        let cleared = json!({ "uri": "file:///work/repo/src/x.rs", "diagnostics": [] });
        assert_eq!(
            findings_from_diagnostics(&cleared, &uris()),
            Some(FileDiagnostics {
                path: "src/x.rs".to_string(),
                findings: Vec::new(),
            })
        );
    }
}
