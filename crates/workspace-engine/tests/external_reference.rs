//! The egress guard for external reference retrieval (spec 51 §5.3, D4, D9).
//!
//! These tests pin the five refusals that must happen before an approval
//! dialog exists: a non-http(s) URL, a secret anywhere in the URL, repository
//! content in the query or fragment, an allowlist entry that is not a host, and
//! an approval that would match a substring. None of them makes a request.

use workspace_engine::{
    Config, ConfigOverlay, Decision, ExternalUrl, InspectionFlags, ReadFileContent, RefusalReason,
    SecretScanner, UrlScheme, inspect, redirect_target,
};

fn hosts(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|entry| entry.to_string()).collect()
}

fn scanner() -> SecretScanner {
    SecretScanner::new(Vec::new())
}

fn inspect_with(url: &str, approved: &[&str]) -> Decision {
    inspect(url, &hosts(approved), &scanner(), &[])
        .expect("the test URL should parse")
        .decision
}

fn is_refused_with(decision: &Decision, expected: impl FnOnce(&RefusalReason) -> bool) -> bool {
    match decision {
        Decision::Refused(reason) => expected(reason),
        _ => false,
    }
}

#[test]
fn only_http_and_https_urls_with_a_real_host_parse() {
    let parsed = ExternalUrl::parse("http://example.com").unwrap();
    assert_eq!(parsed.scheme, UrlScheme::Http);
    assert_eq!(parsed.host, "example.com");
    assert_eq!(parsed.path, "");

    let parsed = ExternalUrl::parse("https://docs.rs/serde/latest/serde/").unwrap();
    assert_eq!(parsed.scheme, UrlScheme::Https);
    assert_eq!(parsed.path, "/serde/latest/serde/");

    let parsed = ExternalUrl::parse("http://127.0.0.1:8080/x?y=1#z").unwrap();
    assert_eq!(parsed.host, "127.0.0.1");
    assert_eq!(parsed.port, Some(8080));
    assert_eq!(parsed.query.as_deref(), Some("y=1"));
    assert_eq!(parsed.fragment.as_deref(), Some("z"));

    let parsed = ExternalUrl::parse("https://[2001:db8::1]/").unwrap();
    assert_eq!(parsed.host, "2001:db8::1");

    let parsed = ExternalUrl::parse("HTTPS://Example.COM:8443/x").unwrap();
    assert_eq!(parsed.scheme, UrlScheme::Https);
    assert_eq!(parsed.host, "example.com");
    assert_eq!(parsed.port, Some(8443));

    let rejected = [
        "",
        "example.com",
        "file:///etc/passwd",
        "ftp://example.com/file",
        "javascript:alert(1)",
        "http://",
        "https:///path",
        "https://user:pass@example.com/",
        "http://exa mple.com/",
        "http://example.com\n",
        "http://example.com\t",
        "http://example.com:0/",
        "http://example.com:99999/",
        "http://example.com:abc/",
        "http://example.com:/",
        "http://exa\\mple.com/",
        "http://[::1/",
        "http://::1/",
        "http://.example.com/",
        "http://example..com/",
        "http://-example.com/",
        "http://*/",
    ];
    for url in rejected {
        assert!(
            ExternalUrl::parse(url).is_err(),
            "should not parse: {url:?}"
        );
        assert!(
            inspect(url, &hosts(&["example.com"]), &scanner(), &[]).is_err(),
            "inspection must refuse an unparsable URL before any approval: {url:?}"
        );
    }
}

#[test]
fn the_host_is_lowercased_and_keeps_a_non_default_port() {
    let parsed = ExternalUrl::parse("HTTPS://Docs.RS.:443/a?b#c").unwrap();
    assert_eq!(parsed.host, "docs.rs");
    assert_eq!(parsed.port, None);
    assert_eq!(parsed.approval_key(), "docs.rs");
    assert_eq!(parsed.request_url(), "HTTPS://Docs.RS.:443/a?b");

    let parsed = ExternalUrl::parse("https://docs.rs:8443/x").unwrap();
    assert_eq!(parsed.port, Some(8443));
    assert_eq!(parsed.approval_key(), "docs.rs:8443");

    let parsed = ExternalUrl::parse("http://127.0.0.1:80/").unwrap();
    assert_eq!(parsed.port, None);
    assert_eq!(parsed.approval_key(), "127.0.0.1");

    let parsed = ExternalUrl::parse("https://[2001:DB8::1]:8443/").unwrap();
    assert_eq!(parsed.host, "2001:db8::1");
    assert_eq!(parsed.approval_key(), "[2001:db8::1]:8443");
}

#[test]
fn a_secret_in_the_query_is_refused_before_any_approval() {
    // Structural detector: 20 upper-case/digit characters after `AKIA`.
    const KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    let scanner = scanner();

    for url in [
        format!("https://example.com/?token={KEY}"),
        format!("https://example.com/{KEY}"),
        "https://example.com/?token=AKIA%49OSFODNN7EXAMPLE".to_string(),
        format!("https://evil.example/?d={KEY}"),
    ] {
        let inspection = inspect(&url, &hosts(&["example.com"]), &scanner, &[])
            .expect("the URL parses; the credential is the refusal");
        assert!(
            is_refused_with(&inspection.decision, |reason| matches!(
                reason,
                RefusalReason::Secret { .. }
            )),
            "a URL carrying a secret must be refused even when its host is approved: {url}"
        );
        let reason = match inspection.decision {
            Decision::Refused(reason) => reason.message(),
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(reason.contains("credential"), "{reason}");
        assert!(
            !reason.contains(KEY),
            "the refusal must not repeat the matched value: {reason}"
        );
    }
}

#[test]
fn repository_content_in_the_query_is_refused_raw_percent_encoded_and_base64() {
    // 27 bytes, above the 16-byte calibration boundary.
    const LINE: &str = "WRITE_REPOSITORY_LINE_ALPHA";
    const BASE64: &str = "V1JJVEVfUkVQT1NJVE9SWV9MSU5FX0FMUEhB";
    const PERCENT: &str = "%57%52%49%54%45%5F%52%45%50%4F%53%49%54%4F%52%59\
                           %5F%4C%49%4E%45%5F%41%4C%50%48%41";
    let files = [ReadFileContent {
        path: "src/widget.rs",
        content: "use std::fmt;\nWRITE_REPOSITORY_LINE_ALPHA\nconst WIDGET_LIMIT = 4096;\nfn rest() {}\n",
    }];
    let scanner = scanner();

    for url in [
        format!("https://example.com/?d={LINE}"),
        format!("https://example.com/?d={PERCENT}"),
        format!("https://example.com/?d={BASE64}"),
        format!("https://example.com/#{LINE}"),
        // A server may read `+` as a space, so the form-encoded spelling of a
        // line with spaces must be refused too.
        "https://example.com/?d=const+WIDGET_LIMIT+=+4096%3B".to_string(),
    ] {
        let inspection =
            inspect(&url, &hosts(&["example.com"]), &scanner, &files).expect("the URL parses");
        assert!(
            is_refused_with(&inspection.decision, |reason| matches!(
                reason,
                RefusalReason::RepositoryContent { .. }
            )),
            "repository content must be refused in raw, percent-encoded and base64 form: {url}"
        );
        let reason = match inspection.decision {
            Decision::Refused(reason) => reason.message(),
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(reason.contains("src/widget.rs"), "{reason}");
    }

    // Below the boundary the line is ordinary text, not someone's source.
    let short = [ReadFileContent {
        path: "src/widget.rs",
        content: "SHORT_LINE_12\n",
    }];
    let inspection = inspect(
        "https://example.com/?d=SHORT_LINE_12",
        &hosts(&["example.com"]),
        &scanner,
        &short,
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);

    // A different file's content is not this task's content.
    let inspection = inspect(
        "https://example.com/?d=WRITE_REPOSITORY_LINE_ALPHA",
        &hosts(&["example.com"]),
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);
}

#[test]
fn a_long_query_or_fragment_is_flagged_not_blocked() {
    let scanner = scanner();
    let short = "a".repeat(128);
    let long = "a".repeat(129);

    let inspection = inspect(
        &format!("https://example.com/?{short}"),
        &hosts(&["example.com"]),
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);
    assert_eq!(inspection.flags, InspectionFlags::default());

    let inspection = inspect(
        &format!("https://example.com/?{long}"),
        &hosts(&["example.com"]),
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);
    assert!(inspection.flags.is_flagged());
    assert_eq!(inspection.flags.long_components.len(), 1);
    assert_eq!(
        inspection.flags.long_components[0].component,
        workspace_engine::UrlComponent::Query
    );
    assert_eq!(inspection.flags.long_components[0].bytes, 129);

    let inspection = inspect(
        &format!("https://example.com/#{long}"),
        &hosts(&["example.com"]),
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(
        inspection.flags.long_components[0].component,
        workspace_engine::UrlComponent::Fragment
    );

    // Flagging is a display rule, so an unapproved host still only asks.
    let inspection = inspect(
        &format!("https://new.example/?{long}"),
        &hosts(&["example.com"]),
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::NeedsApproval);
    assert!(inspection.flags.is_flagged());
}

#[test]
fn the_approval_decision_names_the_host_and_never_a_substring() {
    let approved = ["example.com"];
    assert_eq!(
        inspect_with("https://example.com/docs", &approved),
        Decision::AlreadyAllowed
    );
    assert_eq!(
        inspect_with("https://Example.COM./docs", &approved),
        Decision::AlreadyAllowed
    );
    assert_eq!(
        inspect_with("https://sub.example.com/", &approved),
        Decision::NeedsApproval
    );
    assert_eq!(
        inspect_with("https://example.com.evil/", &approved),
        Decision::NeedsApproval
    );
    assert_eq!(
        inspect_with("https://notexample.com/", &approved),
        Decision::NeedsApproval
    );
    assert_eq!(
        inspect_with("https://example.com:8443/", &approved),
        Decision::NeedsApproval
    );
    assert_eq!(
        inspect_with("http://example.com/", &approved),
        Decision::AlreadyAllowed,
        "the approval is a host, not a scheme"
    );
}

#[test]
fn wildcard_path_and_malformed_allowlist_entries_approve_nothing() {
    let configured = hosts(&[
        "*",
        "*.example.com",
        "example.com/docs",
        "example.com:notaport",
        "example.com:",
        "user@example.com",
        "example.com..",
    ]);
    let scanner = scanner();
    for url in [
        "https://example.com/",
        "https://docs.example.com/",
        "https://evil.example/",
        "https://example.com/docs",
    ] {
        let inspection = inspect(url, &configured, &scanner, &[]).unwrap();
        assert_eq!(
            inspection.decision,
            Decision::NeedsApproval,
            "no malformed entry may approve anything: {url}"
        );
        assert_eq!(
            inspection.ignored_allowlist_entries, configured,
            "every rejected entry is reported, not silently dropped"
        );
    }

    // A well-formed non-default port is a real approval, and only for that port.
    let with_port = hosts(&["example.com:8443"]);
    let inspection = inspect("https://example.com:8443/x", &with_port, &scanner, &[]).unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);
    assert!(inspection.ignored_allowlist_entries.is_empty());
    let inspection = inspect("https://example.com/x", &with_port, &scanner, &[]).unwrap();
    assert_eq!(inspection.decision, Decision::NeedsApproval);
}

#[test]
fn config_host_approvals_are_what_the_guard_reads() {
    let overlay = ConfigOverlay::parse("web_domain_allowlist=Docs.RS.|example.com:8443\n")
        .expect("the overlay should parse");
    let mut config = Config::default();
    config.apply_overlay(overlay);
    assert_eq!(
        config.web_domain_allowlist,
        vec!["docs.rs".to_string(), "example.com:8443".to_string()]
    );

    let scanner = scanner();
    let inspection = inspect(
        "https://docs.rs/serde",
        &config.web_domain_allowlist,
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(inspection.decision, Decision::AlreadyAllowed);

    let inspection = inspect(
        "https://docs.rs:8443/serde",
        &config.web_domain_allowlist,
        &scanner,
        &[],
    )
    .unwrap();
    assert_eq!(
        inspection.decision,
        Decision::NeedsApproval,
        "a per-host entry must not approve the same host on another port"
    );
}

#[test]
fn a_redirect_target_is_named_and_never_followed() {
    let requested = ExternalUrl::parse("https://docs.rs/serde").unwrap();

    let cross = redirect_target(&requested, "https://evil.example/collect");
    assert!(cross.cross_host);
    assert_eq!(cross.url.as_ref().unwrap().host, "evil.example");
    assert_eq!(cross.location, "https://evil.example/collect");
    assert!(!cross.may_follow());

    let same = redirect_target(&requested, "https://docs.rs/other");
    assert!(!same.cross_host);
    assert!(same.url.is_some());
    assert!(
        !same.may_follow(),
        "D10 is stricter than the proposal: no redirect is followed at all"
    );

    let port = redirect_target(&requested, "https://docs.rs:8443/other");
    assert!(port.cross_host);
    assert!(!port.may_follow());

    let relative = redirect_target(&requested, "/other");
    assert!(relative.url.is_none());
    assert!(!relative.cross_host);
    assert!(!relative.may_follow());

    let unsupported = redirect_target(&requested, "file:///etc/passwd");
    assert!(unsupported.url.is_none());
    assert!(!unsupported.may_follow());
    assert!(ExternalUrl::parse("file:///etc/passwd").is_err());
}
