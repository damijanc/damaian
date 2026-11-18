use super::*;
use std::process::Command;
use std::sync::OnceLock;
use workspace_engine::{MemoryCategory, MemoryProposal, MemoryScope, ProposalOrigin, SessionStore};

fn isolated_data_dir() -> &'static Path {
    static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
    DATA_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "damaian-memory-route-data-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("DAMAIAN_DATA_DIR", &dir) };
        dir
    })
}

fn repo_fixture(name: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "damaian-memory-route-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&repo).unwrap();
    // The name goes into the tree so two fixtures never mint the same root
    // commit (and therefore the same project key) within one second.
    fs::write(
        repo.join("README.md"),
        format!("# Memory route fixture {name}\n"),
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "route@example.invalid"],
        vec!["config", "user.name", "Route"],
        vec!["add", "."],
        vec!["commit", "-q", "-m", "fixture"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    repo
}

fn serve() -> (u16, String) {
    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    spawn_with_shell_stack("memory-route-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if let Err(error) = handle_connection(&mut stream, &options) {
                let _ =
                    write_basic_response(&mut stream, 500, "application/json", &json_error(&error));
            }
        }
    })
    .expect("spawn shell");
    (port, token)
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn post(port: u16, token: &str, path: &str, fields: &[(&str, &str)]) -> String {
    let body = fields
        .iter()
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn get(port: u16, token: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn body_json(response: &str) -> serde_json::Value {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or(response);
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
}

fn fixture_proposal(
    session_id: &str,
    task_id: &str,
    scope: MemoryScope,
    statement: &str,
) -> MemoryProposal {
    MemoryProposal::new(
        scope,
        MemoryCategory::Convention,
        statement,
        ProposalOrigin::ModelSuggestion {
            session_id: session_id.to_string(),
            task_id: task_id.to_string(),
        },
        Vec::new(),
        Some(task_id.to_string()),
    )
}

fn engine(data_dir: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    })
}

#[test]
fn the_route_lists_confirms_and_dismisses_exactly_one_proposal_at_a_time() {
    let data_dir = isolated_data_dir();
    let repo = repo_fixture("decisions");
    let sessions = SessionStore::new(data_dir);
    let session = sessions
        .create_session(&repository_id_for_root(&repo), "memory route")
        .unwrap();
    let store = engine(data_dir);
    let first = fixture_proposal(
        &session.id,
        "task_1",
        MemoryScope::Project,
        "Integration tests use nextest.",
    );
    let first = store.memory_store.propose(&repo, first).unwrap().unwrap();
    let second = fixture_proposal(
        &session.id,
        "task_1",
        MemoryScope::Project,
        "Release tags are created on Friday.",
    );
    let second = store.memory_store.propose(&repo, second).unwrap().unwrap();

    let (port, token) = serve();
    let list = get(
        port,
        &token,
        &format!("/api/memory-proposals?session_id={}", session.id),
    );
    assert!(list.starts_with("HTTP/1.1 200"), "{list}");
    let listed = body_json(&list);
    assert_eq!(listed["proposals"].as_array().unwrap().len(), 2);
    assert!(listed["proposals"][0]["sourceLabel"].as_str().is_some());

    let confirmed = post(
        port,
        &token,
        "/api/memory-confirm",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session.id),
            ("proposal_id", &first.id),
            ("scope", "project"),
        ],
    );
    assert!(confirmed.starts_with("HTTP/1.1 200"), "{confirmed}");
    let dismissed = post(
        port,
        &token,
        "/api/memory-dismiss",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session.id),
            ("proposal_id", &second.id),
        ],
    );
    assert!(dismissed.starts_with("HTTP/1.1 200"), "{dismissed}");

    let list = get(
        port,
        &token,
        &format!("/api/memory-proposals?session_id={}", session.id),
    );
    assert_eq!(body_json(&list)["proposals"].as_array().unwrap().len(), 0);
    let project_files = fs::read_dir(data_dir.join("memory").join("project"))
        .unwrap()
        .flatten()
        .count();
    assert_eq!(project_files, 1, "exactly the confirmed entry is stored");
}

#[test]
fn a_user_scope_confirmation_while_disabled_is_refused_and_does_not_enable_it() {
    let data_dir = isolated_data_dir();
    let repo = repo_fixture("user-scope");
    let sessions = SessionStore::new(data_dir);
    let session = sessions
        .create_session(&repository_id_for_root(&repo), "memory user scope")
        .unwrap();
    let store = engine(data_dir);
    let proposal = fixture_proposal(
        &session.id,
        "task_user",
        MemoryScope::User,
        "The user prefers tab indentation.",
    );
    let proposal = store
        .memory_store
        .propose(&repo, proposal)
        .unwrap()
        .unwrap();

    let (port, token) = serve();
    let refused = post(
        port,
        &token,
        "/api/memory-confirm",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session.id),
            ("proposal_id", &proposal.id),
            ("scope", "user"),
        ],
    );
    assert!(refused.starts_with("HTTP/1.1 500"), "{refused}");
    assert!(
        refused.contains("User-scope memory is turned off"),
        "{refused}"
    );
    assert!(!data_dir.join("memory").join("user.jsonl").exists());
    let list = get(
        port,
        &token,
        &format!("/api/memory-proposals?session_id={}", session.id),
    );
    assert_eq!(
        body_json(&list)["proposals"].as_array().unwrap().len(),
        1,
        "a refused confirmation leaves the proposal pending"
    );
}

/// The confirm route takes an id and re-reads the stored proposal; a
/// client-supplied statement or project key has nowhere to go. This is the
/// API shape, not a runtime check the route could forget.
#[test]
fn the_confirm_route_has_no_statement_or_project_key_field() {
    let source = include_str!("memory_api.rs");
    let handler = source
        .split("fn handle_memory_confirm")
        .nth(1)
        .and_then(|rest| rest.split("fn handle_memory_dismiss").next())
        .expect("the confirm handler");
    assert!(!handler.contains("required_form(&form, \"statement\")"));
    assert!(!handler.contains("required_form(&form, \"project_key\")"));
}

#[test]
fn a_session_from_another_repository_cannot_confirm_or_dismiss_here() {
    let data_dir = isolated_data_dir();
    let repo_a = repo_fixture("session-a");
    let repo_b = repo_fixture("session-b");
    let sessions = SessionStore::new(data_dir);
    let session = sessions
        .create_session(&repository_id_for_root(&repo_a), "memory session a")
        .unwrap();
    let store = engine(data_dir);
    let proposal = fixture_proposal(
        &session.id,
        "task_mismatch",
        MemoryScope::Project,
        "Integration tests use nextest.",
    );
    let proposal = store
        .memory_store
        .propose(&repo_a, proposal)
        .unwrap()
        .unwrap();

    let (port, token) = serve();
    for path in ["/api/memory-confirm", "/api/memory-dismiss"] {
        let refused = post(
            port,
            &token,
            path,
            &[
                ("repo", &repo_b.to_string_lossy()),
                ("session_id", &session.id),
                ("proposal_id", &proposal.id),
                ("scope", "project"),
            ],
        );
        assert!(refused.starts_with("HTTP/1.1 500"), "{refused}");
        assert!(refused.contains("another repository"), "{refused}");
    }
    let identity_b = ProjectIdentity::resolve_for_root(&repo_b, &store.git).unwrap();
    let project_b = store.memory_store.project_path(&identity_b.key);
    assert!(
        !project_b.exists(),
        "the other repository must not receive an entry: {}",
        project_b.display()
    );
    let list = get(
        port,
        &token,
        &format!("/api/memory-proposals?session_id={}", session.id),
    );
    assert_eq!(
        body_json(&list)["proposals"].as_array().unwrap().len(),
        1,
        "a mismatched decision leaves the proposal pending"
    );
}

/// Spec 30 Task 5: the inspector's memory read. The manifest names an entry,
/// never its statement (C3), so the panel posts the rows it was shown and
/// this route resolves them. An id owned by another project must read as
/// deleted rather than exposing that project's statement.
#[test]
fn the_context_memory_route_resolves_ids_without_crossing_projects() {
    let data_dir = isolated_data_dir();
    let repo = repo_fixture("context-memory");
    let other = repo_fixture("context-memory-other");
    let sessions = SessionStore::new(data_dir);
    let session = sessions
        .create_session(&repository_id_for_root(&repo), "memory display")
        .unwrap();
    let store = engine(data_dir);
    let identity = ProjectIdentity::resolve_for_root(&repo, &store.git).unwrap();
    let other_identity = ProjectIdentity::resolve_for_root(&other, &store.git).unwrap();
    let visible = store
        .memory_store
        .propose(
            &repo,
            fixture_proposal(
                &session.id,
                "task_display",
                MemoryScope::Project,
                "Integration tests use nextest.",
            ),
        )
        .unwrap()
        .unwrap();
    let visible = store
        .memory_store
        .confirm_proposal(
            &repo,
            Some(&identity),
            &session.id,
            &visible.id,
            UserConfirmation::new(&visible.id),
        )
        .unwrap();
    let hidden = store
        .memory_store
        .propose(
            &other,
            fixture_proposal(
                &session.id,
                "task_other",
                MemoryScope::Project,
                "The other project deploys on Friday.",
            ),
        )
        .unwrap()
        .unwrap();
    let hidden = store
        .memory_store
        .confirm_proposal(
            &other,
            Some(&other_identity),
            &session.id,
            &hidden.id,
            UserConfirmation::new(&hidden.id),
        )
        .unwrap();

    let references = serde_json::json!([
        {
            "entryId": visible.id.clone(),
            "matchReason": {"matchedTerms": ["integration"], "categoryBoost": false},
            "verification": "user_confirmed"
        },
        {
            "entryId": hidden.id.clone(),
            "matchReason": {"matchedTerms": ["friday"], "categoryBoost": false},
            "verification": "user_confirmed"
        }
    ]);
    let (port, token) = serve();
    let response = post(
        port,
        &token,
        "/api/context-memory",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session.id),
            ("memory", &references.to_string()),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let listed = body_json(&response);
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["entryId"], visible.id);
    assert_eq!(entries[0]["statement"], "Integration tests use nextest.");
    assert_eq!(entries[0]["scope"], "project");
    assert_eq!(entries[0]["category"], "convention");
    assert_eq!(entries[0]["provenance"]["type"], "suggestedAndConfirmed");
    assert!(entries[0]["createdAtMs"].is_number());
    assert_eq!(entries[0]["verification"], "user_confirmed");
    assert_eq!(entries[0]["matchReason"]["matchedTerms"][0], "integration");
    assert_eq!(entries[0]["redacted"], false);
    assert_eq!(entries[0]["deleted"], false);
    assert_eq!(entries[1]["deleted"], true);
    assert!(entries[1]["statement"].is_null());
    assert!(
        !response.contains("The other project deploys on Friday."),
        "{response}"
    );
}

/// Spec 30 Task 5: the panel's labels and the stale count are the two
/// pieces the route tests cannot see. This checks the source shape the
/// panel is built from, as the other JS-only assertions in this module do.
#[test]
fn the_inspector_labels_memory_and_reports_its_stale_entries() {
    let app = include_str!("../static/app.js");
    let labels = app
        .split("const CONTEXT_CATEGORY_LABELS = {")
        .nth(1)
        .and_then(|rest| rest.split("};").next())
        .expect("the category labels");
    assert!(labels.contains("memory: \"Memory\""), "{labels}");
    let reasons = app
        .split("const CONTEXT_REASON_LABELS = {")
        .nth(1)
        .and_then(|rest| rest.split("};").next())
        .expect("the reason labels");
    assert!(reasons.contains("memory_no_longer_active:"), "{reasons}");
    let category = app
        .split("function contextCategoryRow")
        .nth(1)
        .and_then(|rest| rest.split("function syncContextRestrictionField").next())
        .expect("the category row");
    assert!(category.contains("evidence_stale"), "{category}");
    assert!(category.contains("context-category-stale"), "{category}");
}

/// The one surface with no browser assertion in the ordinary suite. Run:
///
/// ```sh
/// cargo test -p desktop-shell --lib -- --ignored --exact \
///   memory_route_tests::serves_the_memory_card_for_manual_inspection --nocapture
/// ```
///
/// Then open <http://127.0.0.1:4899/>, set the token in the page and load
/// the printed session. It binds 4899 and seeds two pending proposals.
#[test]
#[ignore]
fn serves_the_memory_card_for_manual_inspection() {
    let data_dir = isolated_data_dir();
    let repo = repo_fixture("memory-walk");
    let session = SessionStore::new(data_dir)
        .create_session(&repository_id_for_root(&repo), "Memory card walk")
        .unwrap();
    let engine = engine(data_dir);
    for (scope, statement, task) in [
        (
            MemoryScope::Project,
            "Integration tests need Docker running",
            "task_walk_project",
        ),
        (
            MemoryScope::User,
            "The user prefers tab indentation",
            "task_walk_user",
        ),
    ] {
        let proposal = fixture_proposal(&session.id, task, scope, statement);
        engine.memory_store.propose(&repo, proposal).unwrap();
    }
    println!("Memory walk server: http://127.0.0.1:4899/");
    println!("  apiToken = \"damaian-memory-walk-token\"");
    println!("  repository = {}", repo.display());
    println!("  session = {}", session.id);
    println!("  data_dir = {}", data_dir.display());
    run_server(ShellOptions {
        port: 4899,
        default_repo: Some(repo.to_string_lossy().to_string()),
        api_token: "damaian-memory-walk-token".to_string(),
    })
    .expect("serve the memory walk");
}

/// Spec 29 §5.4: consent is one proposal at a time. There is no route that
/// accepts a list and no accept-all label in the UI.
#[test]
fn there_is_no_bulk_confirmation_route_or_button() {
    let (port, token) = serve();
    let refused = post(port, &token, "/api/memory-confirm-all", &[("repo", "/tmp")]);
    assert!(refused.starts_with("HTTP/1.1 404"), "{refused}");
    let app = include_str!("../static/app.js").to_lowercase();
    assert!(!app.contains("accept all"));
    assert!(!app.contains("accept-all"));
}
