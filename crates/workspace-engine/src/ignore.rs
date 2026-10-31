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
                // `**/` also matches zero directories, as in gitignore:
                // `src/**/*.rs` matches `src/lib.rs`. Without this the `/`
                // after `**` demands at least one directory.
                if pattern.get(pi + 2) == Some(&b'/')
                    && (vi == 0 || value[vi - 1] == b'/')
                    && inner(pattern, value, pi + 3, vi)
                {
                    return true;
                }
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

/// How many patterns one `{a,b,..}` expansion may produce before the pattern is
/// treated as unsupported. A model-written glob could otherwise multiply a
/// handful of small groups into millions of strings.
const MAX_ALTERNATIVES: usize = 256;

/// [`glob_match`] with `{a,b,c}` alternatives, for `search_content`'s
/// `path_glob` only.
///
/// Git does not expand braces, so `.gitignore` and ignore-rule matching keep
/// using [`glob_match`] and keep treating `{a,b}` literally; an ignore rule that
/// suddenly matched a whole brace group would start hiding files its author did
/// not name. This helper exists for the search tool, whose glob is written by a
/// model and cannot be corrected by editing a file (spec 47 §7.4).
///
/// A group matches any one of its comma-separated alternatives, and may sit
/// inside a longer pattern (`src/{a,b}/*.rs`); several groups are expanded
/// together. Nested groups (`{a,{b,c}}`), an unterminated `{`, and a pattern
/// expanding past [`MAX_ALTERNATIVES`] are unsupported: the pattern is then
/// matched literally, by [`glob_match`], rather than panicking or silently
/// matching nothing for a reason the caller cannot see.
pub fn glob_match_with_alternatives(pattern: &str, value: &str) -> bool {
    let Some(alternatives) = expand_brace_groups(pattern) else {
        return glob_match(pattern, value);
    };
    alternatives
        .iter()
        .any(|alternative| glob_match(alternative, value))
}

/// Every pattern `pattern` expands to, or `None` when it holds a brace group
/// this helper does not support.
fn expand_brace_groups(pattern: &str) -> Option<Vec<String>> {
    let mut expanded = vec![String::new()];
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let (prefix, from_open) = rest.split_at(open);
        if !prefix.is_empty() {
            for alternative in &mut expanded {
                alternative.push_str(prefix);
            }
        }
        let after_open = &from_open[1..];
        let close = after_open.find('}')?;
        let body = &after_open[..close];
        if body.contains(['{', '}']) {
            return None;
        }
        let mut next = Vec::new();
        for alternative in &expanded {
            for choice in body.split(',') {
                let mut combination = alternative.clone();
                combination.push_str(choice);
                next.push(combination);
            }
        }
        if next.len() > MAX_ALTERNATIVES {
            return None;
        }
        expanded = next;
        rest = &after_open[close + 1..];
    }
    if !rest.is_empty() {
        for alternative in &mut expanded {
            alternative.push_str(rest);
        }
    }
    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_star_slash_matches_zero_directories() {
        assert!(glob_match("crates/*/src/**/*.rs", "crates/a/src/lib.rs"));
        assert!(glob_match("**/*.rs", "lib.rs"));
        assert!(glob_match("a/**/b", "a/b"));
    }

    #[test]
    fn double_star_slash_still_matches_nested_directories() {
        assert!(glob_match(
            "crates/*/src/**/*.rs",
            "crates/a/src/finding/x.rs"
        ));
        assert!(glob_match("**/*.rs", "a/b/c.rs"));
        assert!(glob_match("a/**/b", "a/x/y/b"));
    }

    #[test]
    fn double_star_slash_matches_only_at_a_directory_boundary() {
        assert!(!glob_match("a/**/b", "ab"));
        assert!(!glob_match("x**/b", "xb"));
        assert!(!glob_match("crates/*/src/**/*.rs", "crates/a/src.rs"));
    }

    #[test]
    fn a_leading_double_star_ignore_rule_matches_at_the_root() {
        let rules = parse_ignore_patterns(&["**/node_modules".to_string()], "");
        assert!(is_ignored_by_rules(&rules, "node_modules", true));
        assert!(is_ignored_by_rules(&rules, "web/node_modules", true));
    }

    #[test]
    fn a_brace_group_matches_any_one_of_its_alternatives() {
        assert!(glob_match_with_alternatives("{a,b}", "a"));
        assert!(glob_match_with_alternatives("{a,b}", "b"));
        assert!(!glob_match_with_alternatives("{a,b}", "c"));
        // The group is one path segment, not the whole path.
        assert!(!glob_match_with_alternatives("{a,b}", "a/b"));
    }

    #[test]
    fn a_brace_group_matches_inside_a_longer_pattern() {
        assert!(glob_match_with_alternatives("*.{rs,toml}", "lib.rs"));
        assert!(glob_match_with_alternatives("*.{rs,toml}", "Cargo.toml"));
        assert!(!glob_match_with_alternatives("*.{rs,toml}", "lib.md"));

        assert!(glob_match_with_alternatives(
            "src/{a,b}/*.rs",
            "src/a/lib.rs"
        ));
        assert!(glob_match_with_alternatives(
            "src/{a,b}/*.rs",
            "src/b/lib.rs"
        ));
        assert!(!glob_match_with_alternatives(
            "src/{a,b}/*.rs",
            "src/c/lib.rs"
        ));

        // Several groups expand together.
        assert!(glob_match_with_alternatives(
            "{src,tests}/{a,b}.rs",
            "tests/b.rs"
        ));
        assert!(!glob_match_with_alternatives(
            "{src,tests}/{a,b}.rs",
            "docs/a.rs"
        ));
    }

    #[test]
    fn a_brace_group_of_whole_path_globs_matches_each_alternative() {
        assert!(glob_match_with_alternatives("{src/**,docs/**}", "src/a.rs"));
        assert!(glob_match_with_alternatives(
            "{src/**,docs/**}",
            "docs/a.md"
        ));
        assert!(!glob_match_with_alternatives(
            "{src/**,docs/**}",
            "tests/a.rs"
        ));
    }

    /// An unsupported group falls back to literal matching rather than
    /// matching nothing or panicking.
    #[test]
    fn nested_and_unterminated_groups_are_matched_literally() {
        for pattern in ["{a,{b,c}}", "{a,b", "src/{a,{b,c}}/*.rs"] {
            assert_eq!(
                glob_match_with_alternatives(pattern, pattern),
                glob_match(pattern, pattern),
                "{pattern}"
            );
            assert_eq!(
                glob_match_with_alternatives(pattern, "a"),
                glob_match(pattern, "a"),
                "{pattern}"
            );
        }
    }

    /// A pattern with no braces must behave exactly as `glob_match` does, so
    /// every existing `path_glob` keeps its meaning.
    #[test]
    fn a_pattern_without_braces_behaves_exactly_as_before() {
        for pattern in [
            "src/**",
            "src/*.rs",
            "*.md",
            "nomatch/**",
            "crates/*/src/**/*.rs",
            "**/*.rs",
            "a?c",
            "",
        ] {
            for value in [
                "src/a.rs",
                "src/lib.rs",
                "docs/a.md",
                "lib.rs",
                "crates/a/src/lib.rs",
                "abc",
                "",
            ] {
                assert_eq!(
                    glob_match_with_alternatives(pattern, value),
                    glob_match(pattern, value),
                    "{pattern} vs {value}"
                );
            }
        }
    }

    /// Git does not expand braces, so an ignore rule must keep treating them
    /// literally. This is the test that fails if the search helper is ever
    /// wired into the ignore path.
    #[test]
    fn ignore_rules_still_treat_braces_literally() {
        let rules = parse_ignore_patterns(&["{src,docs}/**".to_string()], "");
        assert!(!is_ignored_by_rules(&rules, "src/lib.rs", false));
        assert!(!is_ignored_by_rules(&rules, "docs/a.md", false));
        assert!(!glob_match("{src,docs}/**", "src/lib.rs"));
    }
}
