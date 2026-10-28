//! Inspecting a URL before anything leaves the machine (spec 51 §5.3).
//!
//! A fetch is the first tool that lets the model choose what leaves the
//! machine, so the decision is made here, before an approval dialog exists: a
//! URL that carries a credential or content from a file the task read is
//! refused outright, and the user is never the last line of defence against an
//! exfiltration. This module makes no request and reads no file; the caller
//! passes the `read_file` contents it already holds. Keeping the decision pure
//! is what makes each refusal testable without a server.
//!
//! The other rules that belong to the guard are here too:
//!
//! - only `http` and `https` URLs with a real host parse (D4);
//! - a host approval matches the whole host and the whole port, never a
//!   substring and never a wildcard;
//! - a query or fragment over [`LONG_COMPONENT_THRESHOLD_BYTES`] is flagged for
//!   the dialog, not blocked (proposal §5.3 constraint 2);
//! - a redirect target is named and never followed, whatever its host (D10).

use crate::secret_scanner::SecretScanner;

/// A query or fragment longer than this is broken out for display. A long
/// opaque parameter is the shape of exfiltration and a user scanning a dialog
/// will not spot it inside a long line, but legitimate URLs have long
/// parameters, so the threshold is a display rule and not a block.
pub const LONG_COMPONENT_THRESHOLD_BYTES: usize = 128;

/// The shortest trimmed line from a `read_file` result that counts as
/// repository content (D9). A shorter line matches ordinary words in a URL and
/// would refuse legitimate fetches; calibration was deliberately chosen over
/// checking the whole file, which would match nearly every URL.
pub const MIN_REPOSITORY_LINE_BYTES: usize = 16;

/// The only schemes a fetch accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlScheme {
    Http,
    Https,
}

impl UrlScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    /// The port that is part of the host, not of the approval. A URL written
    /// with it is the same approval as one without (D8).
    pub fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

/// A URL that passed [`ExternalUrl::parse`]. Every field is normalised so that
/// comparing two of them compares what the request will do, not how it was
/// written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalUrl {
    /// The string the model supplied, kept verbatim for the dialog and the
    /// audit record. Never parsed again.
    pub raw: String,
    pub scheme: UrlScheme,
    /// Lower case, without brackets and without a trailing dot.
    pub host: String,
    /// An explicit non-default port only. A default port is dropped so
    /// `https://example.com:443/` and `https://example.com/` are one approval.
    pub port: Option<u16>,
    /// The raw path, including its leading `/` when present.
    pub path: String,
    /// The raw query without `?`, or `None` when there is no non-empty query.
    pub query: Option<String>,
    /// The raw fragment without `#`. Scanned and displayed, but never sent.
    pub fragment: Option<String>,
}

impl ExternalUrl {
    /// Strict parsing (D4): `http`/`https`, non-empty DNS host or IP literal,
    /// optional port, no userinfo, no whitespace or control characters.
    pub fn parse(raw: &str) -> Result<Self, UrlParseError> {
        if raw.is_empty() {
            return Err(UrlParseError::new("the URL is empty"));
        }
        if let Some(byte) = raw.bytes().find(|byte| *byte < 0x21 || *byte == 0x7f) {
            return Err(UrlParseError::new(format!(
                "the URL contains whitespace or a control character (0x{byte:02x}); \
                 a URL must be written as one unbroken string"
            )));
        }
        let Some((scheme, rest)) = raw.split_once("://") else {
            return Err(UrlParseError::new(
                "the URL is not absolute; write the scheme, for example https://example.com",
            ));
        };
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" => UrlScheme::Http,
            "https" => UrlScheme::Https,
            other => {
                return Err(UrlParseError::new(format!(
                    "only http and https URLs can be fetched, not `{other}`"
                )));
            }
        };
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if authority.is_empty() {
            return Err(UrlParseError::new("the URL has no host"));
        }
        if authority.contains('@') {
            return Err(UrlParseError::new(
                "the URL carries credentials in its userinfo; a fetch never sends a credential",
            ));
        }
        if authority.contains('\\') {
            return Err(UrlParseError::new(
                "the URL contains a backslash in its host",
            ));
        }
        let (host, port) = split_authority(authority)?;
        // A default port is part of the host, not a separate approval (D8).
        let port = port.filter(|port| *port != scheme.default_port());
        let mut tail = &rest[authority_end..];
        let fragment = match tail.find('#') {
            Some(index) => {
                let fragment = &tail[index + 1..];
                tail = &tail[..index];
                (!fragment.is_empty()).then(|| fragment.to_string())
            }
            None => None,
        };
        let query = match tail.find('?') {
            Some(index) => {
                let query = &tail[index + 1..];
                tail = &tail[..index];
                (!query.is_empty()).then(|| query.to_string())
            }
            None => None,
        };
        Ok(Self {
            raw: raw.to_string(),
            scheme,
            host,
            port,
            path: tail.to_string(),
            query,
            fragment,
        })
    }

    /// The URL to request: the raw string without its fragment. D4 keeps the
    /// fragment for display and scanning only.
    pub fn request_url(&self) -> String {
        match self.raw.find('#') {
            Some(index) => self.raw[..index].to_string(),
            None => self.raw.clone(),
        }
    }

    /// What `Allow Always` stores and what an allowlist entry is compared
    /// against (D8): the host, plus the port only when it is not the scheme's
    /// default.
    pub fn approval_key(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        }
    }
}

/// Why a string is not a fetchable URL. A parse failure is a refused tool call,
/// never a request (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlParseError {
    reason: String,
}

impl UrlParseError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl std::fmt::Display for UrlParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for UrlParseError {}

fn split_authority(authority: &str) -> Result<(String, Option<u16>), UrlParseError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return Err(UrlParseError::new(
                "the URL's IPv6 host is missing its closing bracket",
            ));
        };
        let host = normalize_ipv6(&rest[..close])
            .ok_or_else(|| UrlParseError::new("the URL's IPv6 host is not a valid address"))?;
        let remainder = &rest[close + 1..];
        let port = match remainder {
            "" => None,
            text => {
                let Some(port) = text.strip_prefix(':') else {
                    return Err(UrlParseError::new(
                        "the URL has an unexpected value after its bracketed host",
                    ));
                };
                Some(parse_port(port)?)
            }
        };
        return Ok((host, port));
    }
    match authority.rfind(':') {
        Some(index) => {
            let host = &authority[..index];
            if host.contains(':') {
                return Err(UrlParseError::new(
                    "an IPv6 host must be written in brackets, for example https://[::1]/",
                ));
            }
            let host = normalize_dns_host(host)
                .ok_or_else(|| UrlParseError::new("the URL's host is not a valid hostname"))?;
            let port = parse_port(&authority[index + 1..])?;
            Ok((host, Some(port)))
        }
        None => {
            let host = normalize_dns_host(authority)
                .ok_or_else(|| UrlParseError::new("the URL's host is not a valid hostname"))?;
            Ok((host, None))
        }
    }
}

fn parse_port(text: &str) -> Result<u16, UrlParseError> {
    if text.is_empty() {
        return Err(UrlParseError::new("the URL has an empty port"));
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UrlParseError::new("the URL's port is not a number"));
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => Err(UrlParseError::new(
            "the URL's port is outside the range 1-65535",
        )),
        Ok(port) => Ok(port),
    }
}

/// Lower case, no trailing dot, a DNS name or dotted-quad only. An underscore
/// is deliberately refused: it is not valid in a host name, and accepting one
/// would make two spellings compare equal in a way the DNS would not.
fn normalize_dns_host(host: &str) -> Option<String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let host = host.to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return None;
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return None;
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
    }
    Some(host)
}

/// Lower case, brackets removed, and structurally an IPv6 literal rather than
/// any colon-bearing string. The address is not otherwise validated: curl
/// rejects a bad literal, and the approval only needs a stable key.
fn normalize_ipv6(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.contains(':') {
        return None;
    }
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
    {
        return None;
    }
    let mut compressions = 0;
    let mut cursor = 0;
    while let Some(index) = host[cursor..].find("::") {
        compressions += 1;
        cursor += index + 2;
    }
    if compressions > 1 {
        return None;
    }
    let groups: Vec<&str> = host.split(':').collect();
    if compressions == 0 && groups.iter().any(|group| group.is_empty()) {
        return None;
    }
    for group in groups.iter().filter(|group| !group.is_empty()) {
        if group.contains('.') {
            if !group.split('.').all(|part| part.parse::<u8>().is_ok()) {
                return None;
            }
        } else if group.len() > 4 || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
    }
    Some(host)
}

/// One `read_file` result from the same task, as the guard needs it: the path
/// for the refusal message and the content for the comparison.
#[derive(Debug, Clone, Copy)]
pub struct ReadFileContent<'a> {
    pub path: &'a str,
    pub content: &'a str,
}

/// Why a URL is refused outright. The wording is what the model sees, so it
/// names the reason without repeating the matched value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
    /// `SecretScanner` matched somewhere in the URL, path included.
    Secret { category: String },
    /// A line from a file this task read appeared in the query or fragment.
    RepositoryContent { path: String },
}

impl RefusalReason {
    pub fn message(&self) -> String {
        match self {
            Self::Secret { category } => format!(
                "the URL carries something that looks like a credential ({category}); \
                 it is refused before any approval is asked"
            ),
            Self::RepositoryContent { path } => format!(
                "the URL carries content from {path}, a file read earlier in this task; \
                 it is refused before any approval is asked"
            ),
        }
    }
}

/// The guard's answer. `NeedsApproval` is the only decision that can reach a
/// dialog; a refusal never does (proposal §5.3 constraint 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    AlreadyAllowed,
    NeedsApproval,
    Refused(RefusalReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlComponent {
    Query,
    Fragment,
}

impl UrlComponent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Fragment => "fragment",
        }
    }
}

/// A query or fragment over the display threshold. The dialog breaks it out and
/// marks it; the fetch still proceeds when the host is approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongComponent {
    pub component: UrlComponent,
    pub bytes: usize,
}

/// Display flags the shell needs. Nothing here changes the decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectionFlags {
    pub long_components: Vec<LongComponent>,
}

impl InspectionFlags {
    pub fn is_flagged(&self) -> bool {
        !self.long_components.is_empty()
    }

    fn for_url(url: &ExternalUrl) -> Self {
        let mut long_components = Vec::new();
        if let Some(query) = &url.query
            && query.len() > LONG_COMPONENT_THRESHOLD_BYTES
        {
            long_components.push(LongComponent {
                component: UrlComponent::Query,
                bytes: query.len(),
            });
        }
        if let Some(fragment) = &url.fragment
            && fragment.len() > LONG_COMPONENT_THRESHOLD_BYTES
        {
            long_components.push(LongComponent {
                component: UrlComponent::Fragment,
                bytes: fragment.len(),
            });
        }
        Self { long_components }
    }
}

/// The guard's complete answer: the parsed URL, the decision, the display
/// flags, and any allowlist entries that were unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub url: ExternalUrl,
    pub decision: Decision,
    pub flags: InspectionFlags,
    /// Config entries that are not a host and therefore approved nothing.
    /// Empty in the normal case. Reported rather than dropped so a mistyped
    /// entry cannot look like a working approval.
    pub ignored_allowlist_entries: Vec<String>,
}

impl Inspection {
    pub fn needs_approval(&self) -> bool {
        matches!(self.decision, Decision::NeedsApproval)
    }

    pub fn is_refused(&self) -> bool {
        matches!(self.decision, Decision::Refused(_))
    }
}

/// A user-scope host allowlist, parsed strictly.
///
/// An entry is one host with an optional non-default port: `docs.rs`,
/// `example.com:8443`, `[::1]:8443`. Wildcards, paths, userinfo and malformed
/// ports are not host approvals and are reported through
/// [`Self::ignored_entries`] instead of being compared as strings. A wildcard
/// entry must never become an approval for anything.
///
/// A port in an entry is compared exactly and is never inferred from the
/// scheme. `Allow Always` writes [`ExternalUrl::approval_key`], which omits a
/// default port, so `https://example.com:443/` needs the `example.com` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostAllowlist {
    entries: Vec<HostEntry>,
    ignored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostEntry {
    host: String,
    port: Option<u16>,
}

impl HostAllowlist {
    pub fn from_config(entries: &[String]) -> Self {
        let mut allowlist = Self::default();
        for raw in entries {
            match parse_host_entry(raw) {
                Some(entry) => {
                    if !allowlist.entries.contains(&entry) {
                        allowlist.entries.push(entry);
                    }
                }
                None => {
                    if !allowlist.ignored.contains(raw) {
                        allowlist.ignored.push(raw.clone());
                    }
                }
            }
        }
        allowlist
    }

    /// Entries that were rejected as host approvals, verbatim.
    pub fn ignored_entries(&self) -> &[String] {
        &self.ignored
    }

    /// Exact host and exact port. An approved `example.com` does not approve
    /// `example.com.evil`, `notexample.com`, a subdomain, or a non-default
    /// port.
    pub fn permits(&self, url: &ExternalUrl) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.host == url.host && entry.port == url.port)
    }
}

fn parse_host_entry(raw: &str) -> Option<HostEntry> {
    let entry = raw.trim();
    if entry.is_empty() {
        return None;
    }
    if entry
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    // A wildcard is not a host. Accepting one would turn an approval into a
    // pattern, and `*.example.com` would silently approve every subdomain.
    if entry.contains(['*', '?', '/', '\\', '#', '@', '%']) {
        return None;
    }
    if entry.contains("://") {
        return None;
    }
    let (host, port) = split_authority(entry).ok()?;
    Some(HostEntry { host, port })
}

/// Inspect `url` against the user-scope host approvals, the secret scanner and
/// the files this task read.
///
/// `files_read` is exactly what `read_file` returned during the task. Files
/// reached through `search_content`, `read_git_diff` or a command are not part
/// of this check (D9), and the caller must not widen `files_read` to cover
/// them.
///
/// Order matters: parsing, then secrets, then repository content, then the
/// allowlist. A refusal wins over an approval, so a URL carrying a credential
/// is refused even when its host is approved (proposal §5.3 constraint 3).
pub fn inspect(
    url: &str,
    allowed_hosts: &[String],
    scanner: &SecretScanner,
    files_read: &[ReadFileContent<'_>],
) -> Result<Inspection, UrlParseError> {
    let parsed = ExternalUrl::parse(url)?;
    let allowlist = HostAllowlist::from_config(allowed_hosts);
    let decision = if let Some(reason) = secret_in_url(&parsed, scanner) {
        Decision::Refused(reason)
    } else if let Some(reason) = repository_content_in_url(&parsed, files_read) {
        Decision::Refused(reason)
    } else if allowlist.permits(&parsed) {
        Decision::AlreadyAllowed
    } else {
        Decision::NeedsApproval
    };
    Ok(Inspection {
        flags: InspectionFlags::for_url(&parsed),
        url: parsed,
        decision,
        ignored_allowlist_entries: allowlist.ignored_entries().to_vec(),
    })
}

fn secret_in_url(url: &ExternalUrl, scanner: &SecretScanner) -> Option<RefusalReason> {
    let mut candidates = vec![url.raw.clone(), percent_decode(&url.raw)];
    for component in [url.query.as_deref(), url.fragment.as_deref()]
        .into_iter()
        .flatten()
    {
        for value in parameter_values(component) {
            if let Some(decoded) = decode_base64(value) {
                candidates.push(String::from_utf8_lossy(&decoded).into_owned());
            }
        }
    }
    for candidate in candidates {
        if let Some(finding) = scanner.scan(&candidate).into_iter().next() {
            return Some(RefusalReason::Secret {
                category: finding.category,
            });
        }
    }
    None
}

fn repository_content_in_url(
    url: &ExternalUrl,
    files_read: &[ReadFileContent<'_>],
) -> Option<RefusalReason> {
    let mut candidates = Vec::new();
    for (component, form_encoded) in [
        (url.query.as_deref(), true),
        (url.fragment.as_deref(), false),
    ]
    .into_iter()
    .filter_map(|(component, form_encoded)| component.map(|text| (text, form_encoded)))
    {
        let decoded = percent_decode(component);
        let mut variants = vec![component.to_string(), decoded.clone()];
        if form_encoded {
            // Many servers decode `+` as a space in a query, so a repository
            // line with spaces can be hidden from the percent-decoded form.
            variants.push(decoded.replace('+', " "));
        }
        for value in variants {
            candidates.push(value.clone());
            if let Some(bytes) = decode_base64(&value) {
                candidates.push(String::from_utf8_lossy(&bytes).into_owned());
            }
            for parameter in parameter_values(&value) {
                candidates.push(parameter.to_string());
                if let Some(bytes) = decode_base64(parameter) {
                    candidates.push(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
        }
    }
    for file in files_read {
        for line in file.content.lines() {
            let line = line.trim();
            if line.len() < MIN_REPOSITORY_LINE_BYTES {
                continue;
            }
            if candidates.iter().any(|candidate| candidate.contains(line)) {
                return Some(RefusalReason::RepositoryContent {
                    path: file.path.to_string(),
                });
            }
        }
    }
    None
}

fn parameter_values(input: &str) -> impl Iterator<Item = &str> {
    input
        .split('&')
        .map(|pair| pair.split_once('=').map_or(pair, |(_, value)| value))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Standard or URL-safe base64, padding optional. A value that is not base64
/// returns `None`, which leaves the raw and percent-decoded candidates in
/// place.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn alphabet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let symbols: Vec<u8> = input.bytes().filter(|byte| *byte != b'=').collect();
    if symbols.is_empty() || symbols.len() % 4 == 1 {
        return None;
    }
    let mut decoded = Vec::with_capacity(symbols.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for symbol in symbols {
        buffer = (buffer << 6) | alphabet(symbol)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    (!decoded.is_empty()).then_some(decoded)
}

/// What a 3xx response means to the guard: never a second request, whichever
/// host it names (D10). The target is returned so the result can state it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectTarget {
    /// The `location` header verbatim, for the result message and the audit.
    pub location: String,
    /// The target when it is itself an absolute http(s) URL.
    pub url: Option<ExternalUrl>,
    /// True when the target names a different host or port from the request.
    pub cross_host: bool,
}

impl RedirectTarget {
    /// The one correct handling, so a caller cannot mistake this type for
    /// permission to fetch. Always false; it exists to make the rule explicit
    /// at the call site.
    pub fn may_follow(&self) -> bool {
        false
    }
}

/// Classify a redirect target. A relative `location` stays unparsed but is
/// still named; an absolute one is parsed so the model can decide whether to
/// issue a new call, which then needs that host's own approval.
pub fn redirect_target(requested: &ExternalUrl, location: &str) -> RedirectTarget {
    let url = ExternalUrl::parse(location).ok();
    let cross_host = url
        .as_ref()
        .is_some_and(|target| target.host != requested.host || target.port != requested.port);
    RedirectTarget {
        location: location.to_string(),
        url,
        cross_host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_handles_valid_invalid_and_utf8_sequences() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%41%4a"), "AJ");
        assert_eq!(percent_decode("%2f%2F"), "//");
        // Not a complete escape, so it stays literal rather than eating a byte.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%e2%82%ac"), "€");
        // `+` is a path/query byte, not form-encoded space.
        assert_eq!(percent_decode("a+b"), "a+b");
    }

    #[test]
    fn base64_decoding_accepts_both_alphabets_and_optional_padding() {
        assert_eq!(decode_base64("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(decode_base64("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert_eq!(decode_base64("aGVsbG8h").as_deref(), Some(&b"hello!"[..]));
        // URL-safe alphabet.
        assert_eq!(decode_base64("Pz8_Pg==").as_deref(), Some(&b"???>"[..]));
        assert_eq!(decode_base64("Pz8_Pg").as_deref(), Some(&b"???>"[..]));
        assert_eq!(decode_base64(""), None);
        assert_eq!(decode_base64("a"), None);
        assert_eq!(decode_base64("not base64"), None);
    }

    #[test]
    fn the_allowlist_only_accepts_hosts_and_optional_ports() {
        let allowlist = HostAllowlist::from_config(&[
            "docs.rs.".to_string(),
            "EXAMPLE.com:8443".to_string(),
            "[::1]:8443".to_string(),
            "*.example.com".to_string(),
            "example.com/docs".to_string(),
            "*".to_string(),
            "example.com:notaport".to_string(),
            "user@example.com".to_string(),
            "docs.rs".to_string(),
        ]);
        assert_eq!(allowlist.entries.len(), 3);
        assert_eq!(
            allowlist.ignored_entries(),
            &[
                "*.example.com".to_string(),
                "example.com/docs".to_string(),
                "*".to_string(),
                "example.com:notaport".to_string(),
                "user@example.com".to_string(),
            ]
        );
        let parsed = ExternalUrl::parse("https://docs.rs/").unwrap();
        assert!(allowlist.permits(&parsed));
        let parsed = ExternalUrl::parse("https://example.com/").unwrap();
        assert!(!allowlist.permits(&parsed));
        let parsed = ExternalUrl::parse("https://example.com:8443/").unwrap();
        assert!(allowlist.permits(&parsed));
        let parsed = ExternalUrl::parse("http://[::1]:8443/").unwrap();
        assert!(allowlist.permits(&parsed));
    }
}
