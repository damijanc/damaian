#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRule {
    pub pattern: String,
    pub base_path: String,
    pub negated: bool,
    pub anchored: bool,
    pub directory_only: bool,
    pub has_slash: bool,
}

pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

pub fn parse_ignore_patterns(patterns: &[String], base_path: &str) -> Vec<IgnoreRule> {
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty() && !pattern.starts_with('#'))
        .map(|raw| {
            let negated = raw.starts_with('!');
            let mut pattern = if negated { &raw[1..] } else { raw };
            let anchored = pattern.starts_with('/');
            if anchored {
                pattern = &pattern[1..];
            }
            let directory_only = pattern.ends_with('/');
            if directory_only {
                pattern = &pattern[..pattern.len() - 1];
            }
            let pattern = normalize_path(pattern);
            IgnoreRule {
                has_slash: pattern.contains('/'),
                pattern,
                base_path: normalize_path(base_path),
                negated,
                anchored,
                directory_only,
            }
        })
        .collect()
}

pub fn is_ignored_by_rules(rules: &[IgnoreRule], relative_path: &str, is_directory: bool) -> bool {
    let mut ignored = false;
    for rule in rules {
        if rule_matches(rule, relative_path, is_directory) {
            ignored = !rule.negated;
        }
    }
    ignored
}

pub fn rule_matches(rule: &IgnoreRule, relative_path: &str, is_directory: bool) -> bool {
    let relative_path = normalize_path(relative_path);
    if !rule.base_path.is_empty()
        && relative_path != rule.base_path
        && !relative_path.starts_with(&format!("{}/", rule.base_path))
    {
        return false;
    }

    if !rule.has_slash && !rule.anchored {
        return relative_path
            .split('/')
            .any(|segment| glob_match(&rule.pattern, segment));
    }

    let full_pattern = [rule.base_path.as_str(), rule.pattern.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");

    if rule.directory_only {
        return (is_directory && glob_match(&full_pattern, &relative_path))
            || relative_path.starts_with(&format!("{full_pattern}/"));
    }
    glob_match(&full_pattern, &relative_path)
}

pub fn glob_match(pattern: &str, value: &str) -> bool {
    fn inner(pattern: &[u8], value: &[u8], pi: usize, vi: usize) -> bool {
        if pi == pattern.len() {
            return vi == value.len();
        }
        if pattern[pi] == b'*' {
            if pi + 1 < pattern.len() && pattern[pi + 1] == b'*' {
                for next in vi..=value.len() {
                    if inner(pattern, value, pi + 2, next) {
                        return true;
                    }
                }
                return false;
            }
            let mut next = vi;
            while next <= value.len() {
                if inner(pattern, value, pi + 1, next) {
                    return true;
                }
                if next == value.len() || value[next] == b'/' {
                    return false;
                }
                next += 1;
            }
            return false;
        }
        if pattern[pi] == b'?' {
            return vi < value.len() && value[vi] != b'/' && inner(pattern, value, pi + 1, vi + 1);
        }
        vi < value.len() && pattern[pi] == value[vi] && inner(pattern, value, pi + 1, vi + 1)
    }

    inner(pattern.as_bytes(), value.as_bytes(), 0, 0)
}
