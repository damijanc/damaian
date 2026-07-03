use crate::hash::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    pub category: String,
    pub start: usize,
    pub end: usize,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    pub text: String,
    pub findings: Vec<SecretFinding>,
}

#[derive(Debug, Clone, Default)]
pub struct SecretScanner {
    custom_patterns: Vec<String>,
}

impl SecretScanner {
    pub fn new(custom_patterns: Vec<String>) -> Self {
        Self { custom_patterns }
    }

    pub fn scan(&self, text: &str) -> Vec<SecretFinding> {
        let mut findings = Vec::new();
        self.scan_private_keys(text, &mut findings);
        self.scan_aws_keys(text, &mut findings);
        self.scan_bearer_tokens(text, &mut findings);
        self.scan_database_passwords(text, &mut findings);
        self.scan_credential_assignments(text, &mut findings);
        self.scan_generic_tokens(text, &mut findings);
        self.scan_custom_patterns(text, &mut findings);
        remove_overlaps(findings)
    }

    pub fn redact(&self, text: &str) -> Redaction {
        let findings = self.scan(text);
        if findings.is_empty() {
            return Redaction {
                text: text.to_string(),
                findings,
            };
        }

        let mut redacted = String::new();
        let mut cursor = 0;
        for finding in &findings {
            redacted.push_str(&text[cursor..finding.start]);
            redacted.push_str(&finding.placeholder);
            cursor = finding.end;
        }
        redacted.push_str(&text[cursor..]);
        Redaction {
            text: redacted,
            findings,
        }
    }

    pub fn contains_secrets(&self, text: &str) -> bool {
        !self.scan(text).is_empty()
    }

    fn add_finding(
        findings: &mut Vec<SecretFinding>,
        category: &str,
        start: usize,
        end: usize,
        value: &str,
    ) {
        if end <= start {
            return;
        }
        let digest = &sha256_hex(value.as_bytes())[..10];
        findings.push(SecretFinding {
            category: category.to_string(),
            start,
            end,
            placeholder: format!("[REDACTED_{}_{digest}]", category.to_uppercase()),
        });
    }

    fn scan_private_keys(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        let mut cursor = 0;
        while let Some(begin) = text[cursor..].find("-----BEGIN ") {
            let start = cursor + begin;
            let rest = &text[start..];
            let header_end = rest.find('\n').unwrap_or(rest.len());
            let header = &rest[..header_end];
            if !header.contains("PRIVATE KEY") {
                cursor = start + "-----BEGIN ".len();
                continue;
            }
            let Some(end_rel) = rest.find("-----END ") else {
                break;
            };
            let end_tail = &rest[end_rel..];
            let marker_body = &end_tail["-----END ".len()..];
            let Some(end_marker_body_end) = marker_body.find("-----") else {
                break;
            };
            let end = start + end_rel + "-----END ".len() + end_marker_body_end + 5;
            Self::add_finding(findings, "private_key", start, end, &text[start..end]);
            cursor = end;
        }
    }

    fn scan_aws_keys(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        for (start, _) in text.match_indices("AKIA") {
            let end = take_while(text, start, |byte| {
                byte.is_ascii_uppercase() || byte.is_ascii_digit()
            });
            if end - start == 20 {
                Self::add_finding(findings, "aws_access_key", start, end, &text[start..end]);
            }
        }
    }

    fn scan_bearer_tokens(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        let lower = text.to_ascii_lowercase();
        let mut cursor = 0;
        while let Some(index) = lower[cursor..].find("bearer ") {
            let token_start = cursor + index + "bearer ".len();
            let token_end = take_while(text, token_start, is_token_byte);
            if token_end - token_start >= 16 {
                Self::add_finding(
                    findings,
                    "bearer_token",
                    token_start,
                    token_end,
                    &text[token_start..token_end],
                );
            }
            cursor = token_end.max(token_start + 1);
        }
    }

    fn scan_database_passwords(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        for scheme in [
            "postgres://",
            "postgresql://",
            "mysql://",
            "mongodb://",
            "redis://",
        ] {
            let mut cursor = 0;
            while let Some(index) = text[cursor..].find(scheme) {
                let url_start = cursor + index;
                let credentials_start = url_start + scheme.len();
                let Some(at_rel) = text[credentials_start..].find('@') else {
                    cursor = credentials_start;
                    continue;
                };
                let at_index = credentials_start + at_rel;
                let Some(colon_rel) = text[credentials_start..at_index].find(':') else {
                    cursor = at_index + 1;
                    continue;
                };
                let password_start = credentials_start + colon_rel + 1;
                if at_index > password_start {
                    Self::add_finding(
                        findings,
                        "database_url",
                        password_start,
                        at_index,
                        &text[password_start..at_index],
                    );
                }
                cursor = at_index + 1;
            }
        }
    }

    fn scan_credential_assignments(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        let keywords = [
            "password",
            "passwd",
            "pwd",
            "secret",
            "api_key",
            "api-key",
            "apikey",
            "token",
            "access_token",
            "access-token",
            "client_secret",
            "client-secret",
        ];
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let lower = line.to_ascii_lowercase();
            for keyword in keywords {
                let mut cursor = 0;
                while let Some(index) = lower[cursor..].find(keyword) {
                    let key_end = cursor + index + keyword.len();
                    let mut value_start = skip_spaces(line, key_end);
                    let Some(separator) = line.as_bytes().get(value_start) else {
                        break;
                    };
                    if *separator != b'=' && *separator != b':' {
                        cursor = key_end;
                        continue;
                    }
                    value_start = skip_spaces(line, value_start + 1);
                    let quote = line.as_bytes().get(value_start).copied();
                    if matches!(quote, Some(b'"' | b'\'')) {
                        value_start += 1;
                    }
                    let value_end = take_while(line, value_start, |byte| {
                        !byte.is_ascii_whitespace() && byte != b'"' && byte != b'\'' && byte != b';'
                    });
                    if value_end - value_start >= 8 {
                        Self::add_finding(
                            findings,
                            "credential_assignment",
                            offset + value_start,
                            offset + value_end,
                            &line[value_start..value_end],
                        );
                    }
                    cursor = value_end.max(key_end + 1);
                }
            }
            offset += line.len();
        }
    }

    fn scan_generic_tokens(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        let prefixes = [
            b"sk".as_slice(),
            b"pk".as_slice(),
            b"rk".as_slice(),
            b"ghp".as_slice(),
            b"github_pat".as_slice(),
            b"xoxb".as_slice(),
            b"xoxp".as_slice(),
        ];
        let bytes = text.as_bytes();
        for index in 0..bytes.len() {
            if index > 0 && is_token_byte(bytes[index - 1]) {
                continue;
            }
            for prefix in prefixes {
                if bytes[index..].starts_with(prefix) {
                    let end = take_while(text, index, is_token_byte);
                    if end - index >= prefix.len() + 20
                        && text.is_char_boundary(index)
                        && text.is_char_boundary(end)
                    {
                        Self::add_finding(
                            findings,
                            "generic_api_key",
                            index,
                            end,
                            &text[index..end],
                        );
                    }
                }
            }
        }
    }

    fn scan_custom_patterns(&self, text: &str, findings: &mut Vec<SecretFinding>) {
        for pattern in &self.custom_patterns {
            if pattern.is_empty() {
                continue;
            }
            let mut cursor = 0;
            while let Some(index) = text[cursor..].find(pattern) {
                let start = cursor + index;
                let end = start + pattern.len();
                Self::add_finding(findings, "custom_secret", start, end, &text[start..end]);
                cursor = end;
            }
        }
    }
}

fn take_while(text: &str, start: usize, predicate: impl Fn(u8) -> bool) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len() && predicate(bytes[index]) {
        index += 1;
    }
    index
}

fn skip_spaces(text: &str, start: usize) -> usize {
    take_while(text, start, |byte| byte == b' ' || byte == b'\t')
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~' | b'+' | b'/' | b'=')
}

fn remove_overlaps(mut findings: Vec<SecretFinding>) -> Vec<SecretFinding> {
    findings.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
    });
    let mut accepted: Vec<SecretFinding> = Vec::new();
    for finding in findings {
        if accepted
            .iter()
            .any(|existing| finding.start < existing.end && finding.end > existing.start)
        {
            continue;
        }
        accepted.push(finding);
    }
    accepted.sort_by_key(|finding| finding.start);
    accepted
}
