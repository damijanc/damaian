use super::*;
use std::process::Command;
use std::sync::OnceLock;
use workspace_engine::SessionStore;

fn isolated_data_dir() -> &'static Path {
    static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
    DATA_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "damaian-branch-route-data-{}",
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

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn ref_exists(repo: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet", reference])
        .status()
        .unwrap()
        .success()
}

fn repo_fixture(name: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "damaian-branch-route-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README.md"), "# Branch route fixture\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["config", "user.email", "route@example.invalid"]);
    git(&repo, &["config", "user.name", "Route"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    repo
}

fn session(repo: &Path, title: &str) -> String {
    SessionStore::new(isolated_data_dir())
        .create_session(&repository_id_for_root(repo), title)
        .unwrap()
        .id
}

fn serve() -> (u16, String) {
    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    spawn_with_shell_stack("branch-route-shell", move || {
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
        .map(|(_, body)| body)
        .unwrap_or(response);
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
}

fn index_bytes(repo: &Path) -> Vec<u8> {
    fs::read(repo.join(".git/index")).unwrap()
}

fn status_text(repo: &Path) -> String {
    git(repo, &["status", "--porcelain=v2"])
}

fn prepare(
    port: u16,
    token: &str,
    repo: &Path,
    session_id: &str,
    operation: &str,
    name: &str,
) -> String {
    post(
        port,
        token,
        "/api/branch-prepare",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", session_id),
            ("operation", operation),
            ("name", name),
        ],
    )
}

fn approve(
    port: u16,
    token: &str,
    repo: &Path,
    session_id: &str,
    prepared: &serde_json::Value,
    confirm_unmerged: bool,
) -> String {
    post(
        port,
        token,
        "/api/branch",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", session_id),
            ("prepare_id", prepared["prepareId"].as_str().unwrap()),
            ("mutation", &prepared["mutation"].to_string()),
            ("approval", "branch"),
            (
                "confirm_unmerged",
                if confirm_unmerged { "true" } else { "false" },
            ),
        ],
    )
}

#[test]
fn the_info_route_reports_state_and_local_branches() {
    let repo = repo_fixture("info");
    git(&repo, &["branch", "side"]);
    let session_id = session(&repo, "Branch info");
    let (port, token) = serve();
    let response = get(
        port,
        &token,
        &format!(
            "/api/branch-info?repo={}&session={}",
            encode(&repo.to_string_lossy()),
            encode(&session_id)
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = body_json(&response);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let current = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(body["current"].as_str(), Some(current.as_str()));
    assert_eq!(body["headOid"].as_str(), Some(head.as_str()));
    assert!(!body["detached"].as_bool().unwrap());
    assert_eq!(body["upstream"], serde_json::Value::Null);
    assert_eq!(body["ahead"], 0);
    assert_eq!(body["behind"], 0);
    assert!(body["inProgressOperation"].is_null());
    assert_eq!(body["conflictedPaths"].as_array().unwrap().len(), 0);
    let branches = body["branches"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|branch| branch.as_str())
        .collect::<Vec<_>>();
    assert!(branches.contains(&current.as_str()), "{branches:?}");
    assert!(branches.contains(&"side"), "{branches:?}");
}

#[test]
fn the_info_route_refuses_a_folder_that_is_not_a_repository() {
    let folder = std::env::temp_dir().join(format!(
        "damaian-branch-route-plain-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&folder).unwrap();
    let session_id = SessionStore::new(isolated_data_dir())
        .create_session(&repository_id_for_root(&folder), "plain folder")
        .unwrap()
        .id;
    let (port, token) = serve();
    let response = get(
        port,
        &token,
        &format!(
            "/api/branch-info?repo={}&session={}",
            encode(&folder.to_string_lossy()),
            encode(&session_id)
        ),
    );
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    let body = body_json(&response);
    assert_eq!(body["code"], "branch_invalid_input");
    assert!(body["message"].as_str().unwrap().contains("Git work tree"));
}

#[test]
fn the_suggestion_route_uses_the_session_or_the_dialog_and_redacts_the_source() {
    let repo = repo_fixture("suggestion");
    let session_id = session(&repo, "Add branch delivery");
    let (port, token) = serve();
    let response = post(
        port,
        &token,
        "/api/branch-suggestion",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session_id),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = body_json(&response);
    assert_eq!(body["name"], "damaian/add-branch-delivery");
    assert!(body["valid"].as_bool().unwrap());
    assert!(!body["exists"].as_bool().unwrap());
    assert!(body["alternative"].is_null());

    let from_dialog = post(
        port,
        &token,
        "/api/branch-suggestion",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session_id),
            ("source", "Fix login bug"),
        ],
    );
    assert_eq!(
        body_json(&from_dialog)["name"],
        "damaian/fix-login-bug",
        "{from_dialog}"
    );

    let secret = post(
        port,
        &token,
        "/api/branch-suggestion",
        &[
            ("repo", &repo.to_string_lossy()),
            ("session_id", &session_id),
            ("source", "rotate AKIAIOSFODNN7EXAMPLE before release"),
        ],
    );
    let name = body_json(&secret)["name"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(!name.contains("akiaiosfodnn7example"), "{name}");
}

#[test]
fn a_branch_change_needs_its_own_branch_approval_not_a_commit_one() {
    let repo = repo_fixture("approval");
    let session_id = session(&repo, "Branch approval");
    let (port, token) = serve();
    let prepared = prepare(
        port,
        &token,
        &repo,
        &session_id,
        "create",
        "damaian/approval",
    );
    assert!(prepared.starts_with("HTTP/1.1 200"), "{prepared}");
    let prepared = body_json(&prepared);
    let refused = post(
        port,
        &token,
        "/api/branch",
        &[
            ("repo", &repo.to_string_lossy()),
            ("prepare_id", prepared["prepareId"].as_str().unwrap()),
            ("mutation", &prepared["mutation"].to_string()),
            ("approval", "commit"),
        ],
    );
    assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
    let refused = body_json(&refused);
    assert_eq!(refused["code"], "approval_required");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("approval=branch")
    );
    assert!(!ref_exists(&repo, "refs/heads/damaian/approval"));
    // A refused approval does not consume the preparation.
    let applied = approve(port, &token, &repo, &session_id, &prepared, false);
    assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
    assert!(ref_exists(&repo, "refs/heads/damaian/approval"));
}

#[test]
fn a_branch_approval_is_bound_to_the_preparing_session() {
    let repo = repo_fixture("session-binding");
    let preparing = session(&repo, "Branch binding");
    let other = session(&repo, "Another session");
    let (port, token) = serve();
    let prepared = prepare(port, &token, &repo, &preparing, "create", "damaian/bound");
    assert!(prepared.starts_with("HTTP/1.1 200"), "{prepared}");
    let prepared = body_json(&prepared);

    let refused = approve(port, &token, &repo, &other, &prepared, false);
    assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
    let body = body_json(&refused);
    assert_eq!(body["code"], "branch_session_mismatch");
    assert!(!ref_exists(&repo, "refs/heads/damaian/bound"));

    let applied = approve(port, &token, &repo, &preparing, &prepared, false);
    assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
    assert!(ref_exists(&repo, "refs/heads/damaian/bound"));
}

#[test]
fn a_code_mode_create_through_the_route_leaves_head_and_the_index_unchanged() {
    let repo = repo_fixture("create");
    let session_id = session(&repo, "Branch create");
    let (port, token) = serve();
    let before_head = git(&repo, &["rev-parse", "HEAD"]);
    let before_branch = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    let before_index = index_bytes(&repo);
    let before_status = status_text(&repo);

    let prepared = prepare(
        port,
        &token,
        &repo,
        &session_id,
        "create",
        "damaian/route-create",
    );
    assert!(prepared.starts_with("HTTP/1.1 200"), "{prepared}");
    let prepared = body_json(&prepared);
    assert!(prepared["canApply"].as_bool().unwrap());
    assert_eq!(prepared["mutation"]["kind"], "createBranch");
    assert_eq!(
        prepared["mutation"]["startPoint"].as_str(),
        Some(before_head.as_str())
    );
    assert_eq!(prepared["suggestion"]["name"], "damaian/route-create");

    let applied = approve(port, &token, &repo, &session_id, &prepared, false);
    assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
    let outcome = body_json(&applied);
    assert_eq!(outcome["branch"], "damaian/route-create");
    assert_eq!(outcome["headBefore"].as_str(), Some(before_head.as_str()));
    assert_eq!(outcome["headAfter"].as_str(), Some(before_head.as_str()));
    assert_eq!(outcome["createdRef"], "refs/heads/damaian/route-create");
    assert_eq!(
        git(&repo, &["rev-parse", "refs/heads/damaian/route-create"]),
        before_head
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(
        git(&repo, &["symbolic-ref", "--short", "HEAD"]),
        before_branch
    );
    assert_eq!(index_bytes(&repo), before_index);
    assert_eq!(status_text(&repo), before_status);
}

#[test]
fn a_switch_through_the_route_moves_head_and_the_working_tree() {
    let repo = repo_fixture("switch");
    let base = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    git(&repo, &["branch", "side"]);
    let side_oid = git(&repo, &["rev-parse", "refs/heads/side"]);
    fs::write(repo.join("README.md"), "# Changed on the base branch\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base change"]);
    let session_id = session(&repo, "Branch switch");
    let (port, token) = serve();

    let prepared = prepare(port, &token, &repo, &session_id, "switch", "side");
    assert!(prepared.starts_with("HTTP/1.1 200"), "{prepared}");
    let prepared = body_json(&prepared);
    assert_eq!(prepared["mutation"]["kind"], "switchBranch");
    let applied = approve(port, &token, &repo, &session_id, &prepared, false);
    assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
    let outcome = body_json(&applied);
    assert_eq!(outcome["branch"], "side");
    assert_eq!(outcome["headAfter"].as_str(), Some(side_oid.as_str()));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), side_oid);
    assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]), "side");
    assert_eq!(
        fs::read_to_string(repo.join("README.md")).unwrap(),
        "# Branch route fixture\n"
    );
    assert_eq!(status_text(&repo), "");
    assert_ne!(base, "side");
}

#[test]
fn the_delete_route_keeps_the_two_step_confirmation() {
    let repo = repo_fixture("delete");
    let base = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    git(&repo, &["branch", "merged"]);
    let session_id = session(&repo, "Branch delete");
    let (port, token) = serve();

    let merged = prepare(port, &token, &repo, &session_id, "delete", "merged");
    assert!(merged.starts_with("HTTP/1.1 200"), "{merged}");
    let merged = body_json(&merged);
    assert_eq!(merged["unmergedCommits"], 0);
    assert_eq!(merged["mutation"]["kind"], "deleteBranch");
    let deleted = approve(port, &token, &repo, &session_id, &merged, false);
    assert!(deleted.starts_with("HTTP/1.1 200"), "{deleted}");
    let deleted = body_json(&deleted);
    let commands = deleted["gitCommands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|command| command.as_str())
        .collect::<Vec<_>>();
    assert!(
        commands.iter().any(|command| command.contains("branch -d")),
        "{commands:?}"
    );
    assert!(!ref_exists(&repo, "refs/heads/merged"));

    // A branch with a commit that is not on the base or a remote.
    git(&repo, &["switch", "-q", "-c", "unmerged"]);
    fs::write(repo.join("unmerged.txt"), "unmerged work\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "unmerged work"]);
    git(&repo, &["switch", "-q", &base]);
    let unmerged = prepare(port, &token, &repo, &session_id, "delete", "unmerged");
    assert!(unmerged.starts_with("HTTP/1.1 200"), "{unmerged}");
    let unmerged = body_json(&unmerged);
    assert_eq!(unmerged["unmergedCommits"], 1);
    assert_eq!(unmerged["unmergedSubjects"][0], "unmerged work");

    let refused = approve(port, &token, &repo, &session_id, &unmerged, false);
    assert!(refused.starts_with("HTTP/1.1 409"), "{refused}");
    let refused = body_json(&refused);
    assert_eq!(refused["code"], "branch_unmerged_confirmation_required");
    assert!(refused["error"].as_str().is_some());
    assert!(refused["message"].as_str().unwrap().contains("1 commit"));
    assert!(ref_exists(&repo, "refs/heads/unmerged"));
    // The record survives the refusal, so the second decision can still be
    // made without preparing again.
    let still_refused = approve(port, &token, &repo, &session_id, &unmerged, false);
    assert!(still_refused.starts_with("HTTP/1.1 409"), "{still_refused}");
    let confirmed = approve(port, &token, &repo, &session_id, &unmerged, true);
    assert!(confirmed.starts_with("HTTP/1.1 200"), "{confirmed}");
    let confirmed = body_json(&confirmed);
    let commands = confirmed["gitCommands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|command| command.as_str())
        .collect::<Vec<_>>();
    assert!(
        commands.iter().any(|command| command.contains("branch -D")),
        "{commands:?}"
    );
    assert!(!ref_exists(&repo, "refs/heads/unmerged"));

    let current = prepare(
        port,
        &token,
        &repo,
        &session_id,
        "delete",
        &git(&repo, &["symbolic-ref", "--short", "HEAD"]),
    );
    assert!(current.starts_with("HTTP/1.1 409"), "{current}");
    let current = body_json(&current);
    assert_eq!(current["code"], "branch_refused");
    assert!(
        current["message"]
            .as_str()
            .unwrap()
            .contains("current branch")
    );
}

#[test]
fn ask_mode_is_refused_before_a_record_or_a_ref_exists() {
    let repo = repo_fixture("ask");
    let session_id = session(&repo, "Branch ask");
    SessionStore::new(isolated_data_dir())
        .set_session_mode(&session_id, SessionMode::Ask, "test")
        .unwrap();
    let (port, token) = serve();
    let refused = prepare(port, &token, &repo, &session_id, "create", "damaian/ask");
    assert!(refused.starts_with("HTTP/1.1 409"), "{refused}");
    let refused = body_json(&refused);
    assert_eq!(refused["code"], "branch_refused");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("Code mode only")
    );
    assert!(!refused["preparation"]["canApply"].as_bool().unwrap());
    assert!(!ref_exists(&repo, "refs/heads/damaian/ask"));
}

#[test]
fn merge_and_rebase_preparation_leave_the_index_head_and_status_identical() {
    let repo = repo_fixture("merge");
    let base = git(&repo, &["symbolic-ref", "--short", "HEAD"]);
    let base_oid = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["switch", "-q", "-c", "feature"]);
    fs::write(repo.join("feature.txt"), "feature work\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "feature work"]);
    git(&repo, &["switch", "-q", &base]);
    let session_id = session(&repo, "Branch merge");
    let (port, token) = serve();

    let before_head = git(&repo, &["rev-parse", "HEAD"]);
    let before_index = index_bytes(&repo);
    let before_status = status_text(&repo);
    let response = get(
        port,
        &token,
        &format!(
            "/api/merge-preparation?repo={}&session={}&mode=merge&base=feature",
            encode(&repo.to_string_lossy()),
            encode(&session_id)
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let report = body_json(&response);
    assert!(report["conflictPredictionAvailable"].as_bool().unwrap());
    assert_eq!(report["mergeTreeExitStatus"], 0);
    assert!(report["fastForward"].as_bool().unwrap());
    assert_eq!(report["incomingCommits"].as_array().unwrap().len(), 1);
    assert_eq!(report["conflictedPaths"].as_array().unwrap().len(), 0);
    assert!(
        report["changedFiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "feature.txt")
    );
    assert!(report["problem"].is_null());
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(index_bytes(&repo), before_index);
    assert_eq!(status_text(&repo), before_status);
    assert!(!repo.join(".git/MERGE_HEAD").exists());

    git(&repo, &["switch", "-q", "feature"]);
    let before_head = git(&repo, &["rev-parse", "HEAD"]);
    let rebase = get(
        port,
        &token,
        &format!(
            "/api/merge-preparation?repo={}&session={}&mode=rebase&base={base}",
            encode(&repo.to_string_lossy()),
            encode(&session_id)
        ),
    );
    assert!(rebase.starts_with("HTTP/1.1 200"), "{rebase}");
    let rebase = body_json(&rebase);
    assert_eq!(rebase["onto"].as_str(), Some(base.as_str()));
    assert_eq!(rebase["ontoOid"].as_str(), Some(base_oid.as_str()));
    assert_eq!(rebase["commitsToReplay"].as_array().unwrap().len(), 1);
    assert!(!rebase["conflictPredictionAvailable"].as_bool().unwrap());
    assert!(
        rebase["conflictPredictionUnavailableReason"]
            .as_str()
            .unwrap()
            .contains("would require replaying")
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before_head);
    assert!(!repo.join(".git/REBASE_HEAD").exists());

    let unknown = get(
        port,
        &token,
        &format!(
            "/api/merge-preparation?repo={}&session=missing-session&mode=merge&base={base}",
            encode(&repo.to_string_lossy()),
        ),
    );
    assert!(unknown.starts_with("HTTP/1.1 400"), "{unknown}");
    assert_eq!(body_json(&unknown)["code"], "branch_unknown_session");

    let refused_mode = get(
        port,
        &token,
        &format!(
            "/api/merge-preparation?repo={}&session={}&mode=cherry-pick&base={base}",
            encode(&repo.to_string_lossy()),
            encode(&session_id)
        ),
    );
    assert!(refused_mode.starts_with("HTTP/1.1 400"), "{refused_mode}");
    assert_eq!(body_json(&refused_mode)["code"], "branch_invalid_mode");
}

#[test]
fn no_route_executes_a_merge_rebase_or_cherry_pick() {
    let repo = repo_fixture("no-execute");
    let session_id = session(&repo, "No execute");
    let (port, token) = serve();
    for path in [
        "/api/merge",
        "/api/rebase",
        "/api/branch-merge",
        "/api/branch-rebase",
        "/api/cherry-pick",
    ] {
        let response = post(
            port,
            &token,
            path,
            &[
                ("repo", &repo.to_string_lossy()),
                ("session_id", &session_id),
                ("base", "main"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 404"), "{path}: {response}");
    }
    let before_head = git(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(status_text(&repo), "");
    assert!(!repo.join(".git/MERGE_HEAD").exists());
}
