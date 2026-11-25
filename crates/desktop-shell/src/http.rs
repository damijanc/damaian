//! The shell's HTTP primitives: reading a request, writing responses and
//! server-sent events, CORS and the API token.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: HashMap<String, String>,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

impl Request {
    pub(crate) fn param(&self, name: &str) -> Option<String> {
        self.query.get(name).cloned()
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

pub(crate) fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 8192];
    loop {
        let read = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err("request header too large".to_string());
        }
    }
    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "malformed request".to_string())?
        + 4;
    let header = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = header.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let parts = request_line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err("malformed request line".to_string());
    }
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<HashMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }

    let (path, query) = split_path_query(parts[1]);
    let body = String::from_utf8_lossy(
        &buffer
            [header_end..header_end + content_length.min(buffer.len().saturating_sub(header_end))],
    )
    .to_string();
    Ok(Request {
        method: parts[0].to_string(),
        path,
        query,
        headers,
        body,
    })
}

fn split_path_query(raw: &str) -> (String, HashMap<String, String>) {
    let (path, query) = raw.split_once('?').unwrap_or((raw, ""));
    (path.to_string(), parse_form(query))
}

pub(crate) fn required_param(request: &Request, name: &str) -> Result<String, String> {
    request
        .param(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing query parameter: {name}"))
}

pub(crate) fn required_form(form: &HashMap<String, String>, name: &str) -> Result<String, String> {
    form.get(name)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("missing form field: {name}"))
}

pub(crate) fn api_request_requires_token(path: &str) -> bool {
    path.starts_with("/api/")
}

pub(crate) fn parse_path_list(value: &str) -> Result<Vec<String>, String> {
    let paths = parse_optional_path_list(value);
    if paths.is_empty() {
        Err("at least one patch file must be selected".to_string())
    } else {
        Ok(paths)
    }
}

pub(crate) fn parse_optional_path_list(value: &str) -> Vec<String> {
    value
        .split(['\n', '|'])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.to_string())
        .collect()
}

pub(crate) fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

pub(crate) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => output.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                if let (Some(high), Some(low)) =
                    (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
                {
                    output.push(high * 16 + low);
                    index += 3;
                    continue;
                }
                output.push(bytes[index]);
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn index_html() -> String {
    INDEX_HTML.to_string()
}

pub(crate) fn write_response(
    stream: &mut dyn Write,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    write_response_with_extra_headers(stream, request, status, content_type, body, "")
}

pub(crate) fn write_basic_response(
    stream: &mut dyn Write,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\nconnection: close\r\n\r\n{body}",
        status_text(status),
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

pub(crate) fn write_binary_response(
    stream: &mut dyn Write,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let header = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\n{cors_headers}connection: close\r\n\r\n",
        status_text(status),
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| error.to_string())
}

pub(crate) fn write_preflight_response(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    if allowed_cors_origin(request).is_none() {
        return write_response(
            stream,
            request,
            403,
            "application/json",
            &json_error("forbidden"),
        );
    }
    write_response_with_extra_headers(stream, request, 204, "text/plain; charset=utf-8", "", "")
}

pub(crate) fn write_response_with_extra_headers(
    stream: &mut dyn Write,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &str,
    extra_headers: &str,
) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\n{cors_headers}{extra_headers}connection: close\r\n\r\n{body}",
        status_text(status),
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

pub(crate) fn write_event_stream_headers(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-store\r\n{cors_headers}connection: close\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

pub(crate) fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn cors_headers(request: &Request) -> String {
    allowed_cors_origin(request)
        .map(|origin| {
            format!(
                "access-control-allow-origin: {origin}\r\naccess-control-allow-methods: GET, POST, OPTIONS\r\naccess-control-allow-headers: content-type, x-damaian-api-token\r\nvary: origin\r\n"
            )
        })
        .unwrap_or_default()
}

pub(crate) fn allowed_cors_origin(request: &Request) -> Option<&str> {
    let origin = request.header("origin")?;
    let allowed = matches!(
        origin,
        "http://tauri.localhost"
            | "https://tauri.localhost"
            | "tauri://localhost"
            | "http://localhost:4765"
            | "http://127.0.0.1:4765"
    );
    allowed.then_some(origin)
}

pub(crate) fn require_api_token(request: &Request, expected_token: &str) -> Result<(), String> {
    if request.header("x-damaian-api-token") == Some(expected_token) {
        Ok(())
    } else {
        Err("unauthorized API request".to_string())
    }
}

pub(crate) fn generate_api_token() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("secure random token generation failed");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn write_sse_event<W: Write + ?Sized>(
    out: &mut W,
    event: &str,
    data: &str,
) -> Result<(), String> {
    out.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())
        .and_then(|_| out.flush())
        .map_err(|error| error.to_string())
}

/// How often the handler writes to a silent stream. This is the only thing that
/// reveals a client that has gone away mid-turn: with no data flowing there is
/// nothing else to fail on. Note that on macOS the first write after the peer
/// closes usually succeeds into the kernel buffer, so detection takes one or two
/// of these rather than being instant.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
