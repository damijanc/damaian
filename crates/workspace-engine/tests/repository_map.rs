//! Repository map and monorepo boundaries, per
//! `docs/specs/24_repository_map_and_monorepo_boundaries/proposal.md`.
//!
//! Task 1 pins root detection. It is a pure function of the index's file
//! and skip lists (`context.md` §3), so only the test that ties the manifest
//! list to `detect_project_commands` touches the filesystem.
//!
//! Task 2 pins the command location pair (`context.md` §1–§2): tokens
//! resolve against the working directory, containment and the allowlist
//! are judged against the repository root.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::indexer::SkippedFile;
use workspace_engine::{
    CancelToken, CommandPolicy, CommandRisk, CommandStore, Config, DetectedRoot, ExcludedPath,
    ExclusionReason, MAX_ROOT_DEPTH, PROJECT_MANIFESTS, RootDetection, RootEvidence,
    WorkspaceEngine, detect_roots, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-repomap-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn ignored(path: &str) -> SkippedFile {
    SkippedFile {
        path: path.to_string(),
        reason: "ignored".to_string(),
    }
}

fn root(path: &str, manifest: &str) -> DetectedRoot {
    DetectedRoot {
        path: path.to_string(),
        detected_by: RootEvidence::Manifest {
            path: manifest.to_string(),
        },
    }
}

fn repository_root_without_manifest() -> DetectedRoot {
    DetectedRoot {
        path: String::new(),
        detected_by: RootEvidence::RepositoryRoot,
    }
}

fn excluded(path: &str, reason: ExclusionReason) -> ExcludedPath {
    ExcludedPath {
        path: path.to_string(),
        reason,
    }
}

/// A Cargo workspace with two members beside two npm packages, plus two
/// files whose names only contain a manifest's name.
const MONOREPO: &[&str] = &[
    "Cargo.toml",
    "README.md",
    "crates/cli/Cargo.toml",
    "crates/cli/src/main.rs",
    "crates/engine/Cargo.toml",
    "crates/engine/src/lib.rs",
    "docs/package.json.md",
    "examples/Cargo.toml.orig",
    "packages/api/package.json",
    "packages/api/src/index.ts",
    "packages/web/package.json",
    "packages/web/src/index.ts",
];

#[test]
fn a_repository_with_no_manifest_has_exactly_one_root() {
    let detection = detect_roots(["README.md", "src/notes.txt"], &[]);
    assert_eq!(
        detection,
        RootDetection {
            roots: vec![repository_root_without_manifest()],
            excluded: vec![],
        }
    );
}

#[test]
fn nested_roots_are_kept_and_each_names_its_manifest() {
    let detection = detect_roots(MONOREPO.iter().copied(), &[]);
    assert_eq!(
        detection.roots,
        vec![
            root("", "Cargo.toml"),
            root("crates/cli", "crates/cli/Cargo.toml"),
            root("crates/engine", "crates/engine/Cargo.toml"),
            root("packages/api", "packages/api/package.json"),
            root("packages/web", "packages/web/package.json"),
        ]
    );
    assert!(detection.excluded.is_empty());
}

#[test]
fn vendor_and_build_output_directories_are_never_roots_and_are_recorded() {
    let files = [
        "package.json",
        // Indexed only because a user removed `node_modules/` from
        // `ignore_patterns`. It still must not become a root (§5.2).
        "node_modules/left-pad/package.json",
        "node_modules/left-pad/index.js",
        "packages/api/node_modules/react/package.json",
        "packages/api/package.json",
    ];
    let skipped = [
        ignored("vendor"),
        ignored("target"),
        ignored("dist"),
        ignored("build"),
        ignored("packages/api/dist"),
        // Ignored, but not a vendor directory: not listed (context.md §11).
        ignored("coverage"),
        ignored(".env.local"),
    ];
    let detection = detect_roots(files, &skipped);
    assert_eq!(
        detection.roots,
        vec![
            root("", "package.json"),
            root("packages/api", "packages/api/package.json"),
        ]
    );
    use ExclusionReason::Vendor;
    assert_eq!(
        detection.excluded,
        vec![
            excluded("build", Vendor),
            excluded("dist", Vendor),
            excluded("node_modules", Vendor),
            excluded("packages/api/dist", Vendor),
            excluded("packages/api/node_modules", Vendor),
            excluded("target", Vendor),
            excluded("vendor", Vendor),
        ]
    );
}

#[test]
fn roots_below_the_depth_ceiling_are_recorded_not_dropped() {
    assert_eq!(MAX_ROOT_DEPTH, 6, "context.md §11 records this value");
    let detection = detect_roots(
        [
            "a/b/c/d/e/f/package.json",
            "a/b/c/d/e/f/g/package.json",
            "a/b/c/d/e/f/g/h/Cargo.toml",
        ],
        &[],
    );
    assert_eq!(
        detection.roots,
        vec![
            repository_root_without_manifest(),
            root("a/b/c/d/e/f", "a/b/c/d/e/f/package.json"),
        ]
    );
    use ExclusionReason::BelowDepthCeiling;
    assert_eq!(
        detection.excluded,
        vec![
            excluded("a/b/c/d/e/f/g", BelowDepthCeiling),
            excluded("a/b/c/d/e/f/g/h", BelowDepthCeiling),
        ]
    );
}

#[test]
fn detection_does_not_depend_on_input_order() {
    // The repository root holds two manifests. Its evidence is the first in
    // `PROJECT_MANIFESTS` order, whichever one the walk met first.
    let forward: Vec<&str> = [&["package.json"][..], MONOREPO].concat();
    let mut backward = forward.clone();
    backward.reverse();

    let a = detect_roots(
        forward.iter().copied(),
        &[ignored("target"), ignored("dist")],
    );
    let b = detect_roots(
        backward.iter().copied(),
        &[ignored("dist"), ignored("target")],
    );

    assert_eq!(a, b);
    assert_eq!(a.roots[0], root("", "package.json"));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn detection_serialises_in_the_shape_the_map_file_and_the_ui_read() {
    let value = serde_json::to_value(root("packages/api", "packages/api/package.json")).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "path": "packages/api",
            "detectedBy": { "kind": "manifest", "path": "packages/api/package.json" }
        })
    );
    let value = serde_json::to_value(excluded(
        "a/b/c/d/e/f/g",
        ExclusionReason::BelowDepthCeiling,
    ))
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "path": "a/b/c/d/e/f/g", "reason": "belowDepthCeiling" })
    );
}

#[test]
fn every_root_manifest_gives_its_directory_a_command() {
    for manifest in PROJECT_MANIFESTS {
        let dir = temp_dir(manifest);
        let content = if manifest == "package.json" {
            "{\"scripts\":{\"test\":\"node test.js\"}}"
        } else {
            ""
        };
        fs::write(dir.join(manifest), content).unwrap();
        let policy = CommandPolicy::new(Config {
            data_dir: dir.join(".damaian"),
            enable_index_watcher: false,
            ..Config::default()
        });
        let commands = policy
            .detect_project_commands(&dir)
            .expect("detection should read the fixture");
        let _ = fs::remove_dir_all(&dir);
        assert!(
            !commands.is_empty(),
            "{manifest} makes a directory a root but gives it nothing to run (context.md §10)"
        );

        let path = format!("pkg/{manifest}");
        let detection = detect_roots([path.as_str()], &[]);
        assert_eq!(detection.roots.last(), Some(&root("pkg", &path)));
    }
}

const OUTSIDE_ROOT_REASON: &str = "Command references a path outside the selected repository";

/// A repository with two npm packages, its data directory and user config
/// inside the temp dir so nothing touches the real application state.
struct Monorepo {
    root: PathBuf,
    api: PathBuf,
    web: PathBuf,
    data_dir: PathBuf,
    user_config: PathBuf,
}

impl Monorepo {
    fn new(name: &str, user: &str) -> Self {
        let root = temp_dir(name);
        let api = root.join("packages/api");
        let web = root.join("packages/web");
        for package in [&api, &web] {
            fs::create_dir_all(package.join("src")).unwrap();
            fs::write(
                package.join("package.json"),
                "{\"scripts\":{\"test\":\"node test.js\"}}",
            )
            .unwrap();
        }
        fs::write(web.join("src/index.ts"), "export {};\n").unwrap();
        let data_dir = root.join(".damaian");
        let user_config = data_dir.join("config").join("user.conf");
        fs::create_dir_all(user_config.parent().unwrap()).unwrap();
        let user = user.replace("<id>", &repository_id_for_root(&root));
        fs::write(&user_config, user).unwrap();
        Self {
            root,
            api,
            web,
            data_dir,
            user_config,
        }
    }

    fn base(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            shell: "/usr/bin/true".to_string(),
            ..Config::default()
        }
    }

    /// The config a repository-scoped engine loads: user config, then the
    /// `Allow Always` grants for this repository's id.
    fn load(&self) -> Config {
        Config::load_scoped(
            self.base(),
            Some(&self.user_config),
            None,
            None,
            Some(&self.root),
        )
        .expect("fixture config should load")
        .0
    }

    fn policy(&self) -> CommandPolicy {
        CommandPolicy::new(self.load())
    }
}

impl Drop for Monorepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_root_qualified_grant_authorises_its_directory_only() {
    let repo = Monorepo::new(
        "qualified",
        "command_allowlist.<id>=cd packages/api && npm test\n",
    );
    let policy = repo.policy();

    let api = policy.classify_at("npm test", &repo.root, &repo.api);
    assert_eq!(api.risk, CommandRisk::Low);
    assert!(!api.requires_approval);

    let web = policy.classify_at("npm test", &repo.root, &repo.web);
    assert_eq!(web.risk, CommandRisk::Medium);
    assert!(web.requires_approval);

    let root = policy.classify_at("npm test", &repo.root, &repo.root);
    assert!(root.requires_approval);
    assert_ne!(root.risk, CommandRisk::Low);
}

#[test]
fn a_plain_allowlist_entry_matches_only_at_the_repository_root() {
    let repo = Monorepo::new("plain", "command_allowlist=npm test\n");
    let policy = repo.policy();

    let root = policy.classify_at("npm test", &repo.root, &repo.root);
    assert_eq!(root.risk, CommandRisk::Low);
    assert!(!root.requires_approval);

    // context.md §2, "What this narrows".
    let api = policy.classify_at("npm test", &repo.root, &repo.api);
    assert_eq!(api.risk, CommandRisk::Medium);
    assert!(api.requires_approval);
}

#[test]
fn a_qualified_entry_cannot_be_matched_by_typing_it() {
    let repo = Monorepo::new(
        "typed",
        "command_allowlist.<id>=cd packages/api && npm test\n",
    );
    let typed = repo
        .policy()
        .classify_at("cd packages/api && npm test", &repo.root, &repo.root);
    assert_eq!(typed.risk, CommandRisk::High);
    assert!(typed.requires_approval);
}

#[test]
fn a_working_directory_outside_the_repository_matches_nothing_and_needs_approval() {
    let repo = Monorepo::new("outside-wd", "command_allowlist=ls\n");
    let outside = temp_dir("outside-wd-elsewhere");
    let classification = repo.policy().classify_at("ls", &repo.root, &outside);
    let _ = fs::remove_dir_all(&outside);
    assert!(classification.requires_approval);
    assert_ne!(classification.risk, CommandRisk::Low);
}

#[test]
fn allow_always_in_a_sub_root_writes_a_qualified_grant_under_the_repository_id() {
    let repo = Monorepo::new("allow-always", "");
    let engine = WorkspaceEngine::new(repo.load());
    let proposal = engine
        .validation_orchestrator
        .propose_command_at(&repo.root, &repo.api, "npm test", "Run the API tests")
        .unwrap();
    assert!(proposal.requires_approval);

    let path = engine
        .validation_orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();

    assert_eq!(path, repo.user_config);
    let written = fs::read_to_string(&path).unwrap();
    let root_id = repository_id_for_root(&repo.root);
    let api_id = repository_id_for_root(&repo.api);
    assert!(
        written.contains(&format!(
            "command_allowlist.{root_id}=cd packages/api && npm test"
        )),
        "the grant belongs to the repository, qualified with its directory: {written}"
    );
    assert!(
        !written.contains(&api_id),
        "nothing loads config under the sub-directory's id: {written}"
    );

    let policy = repo.policy();
    let api = policy.classify_at("npm test", &repo.root, &repo.api);
    assert_eq!(api.risk, CommandRisk::Low);
    assert!(!api.requires_approval);
    let web = policy.classify_at("npm test", &repo.root, &repo.web);
    assert!(web.requires_approval);
}

#[test]
fn paths_resolve_from_the_working_directory_and_are_bounded_by_the_repository() {
    let repo = Monorepo::new("containment", "");
    let policy = repo.policy();

    let sibling = policy.classify_at("cat ../web/src/index.ts", &repo.root, &repo.api);
    assert!(
        !sibling
            .reasons
            .iter()
            .any(|reason| reason == OUTSIDE_ROOT_REASON),
        "a sibling root is inside the repository: {:?}",
        sibling.reasons
    );

    let escape = policy.classify_at("cat ../../../../etc/passwd", &repo.root, &repo.api);
    assert!(escape.requires_approval);
    assert!(
        escape
            .reasons
            .iter()
            .any(|reason| reason == OUTSIDE_ROOT_REASON),
        "{:?}",
        escape.reasons
    );

    // A root never widens anything: what escapes the repository from the
    // root still escapes it from a sub-root.
    let absolute = policy.classify_at("ls /etc", &repo.root, &repo.api);
    assert!(absolute.requires_approval);
    assert_eq!(absolute.risk, CommandRisk::Medium);
}

#[test]
fn a_proposal_stored_before_repository_roots_loads_at_its_working_directory() {
    let repo = Monorepo::new("legacy-proposal", "");
    let working_directory = repo.root.to_string_lossy().to_string();
    let mut raw = String::from("DAMAIAN_COMMAND_PROPOSAL_V1\n");
    for (name, value) in [
        ("ID", "cmdprop_legacy"),
        ("COMMAND", "npm test"),
        ("WORKING_DIRECTORY", working_directory.as_str()),
        ("REASON", "Run the tests"),
        ("RISK", "medium"),
        ("REQUIRES_APPROVAL", "true"),
        ("BLOCKED", "false"),
        ("EXPECTED_EFFECTS", "Runs project validation"),
        ("MAY_USE_NETWORK", "false"),
        ("REASONS", "Validation command"),
        ("CREATED_AT_MS", "1"),
        ("STATUS", "pending"),
    ] {
        raw.push_str(&format!("{name} {}\n{value}\n", value.len()));
    }
    raw.push_str("END_COMMAND_PROPOSAL\n");
    let pending = repo.data_dir.join("commands/pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(pending.join("cmdprop_legacy.dcmd"), raw).unwrap();

    let proposal = CommandStore::new(&repo.data_dir)
        .load_proposal("cmdprop_legacy")
        .unwrap();

    assert_eq!(proposal.working_directory, working_directory);
    assert_eq!(proposal.repository_root, working_directory);
}

#[test]
fn a_proposal_round_trips_its_location_pair() {
    let repo = Monorepo::new("round-trip", "");
    let engine = WorkspaceEngine::new(repo.load());
    let proposal = engine
        .validation_orchestrator
        .propose_command_at(&repo.root, &repo.api, "npm test", "Run the API tests")
        .unwrap();
    let loaded = engine
        .validation_orchestrator
        .load_proposal(&proposal.id)
        .unwrap();
    assert_eq!(loaded, proposal);
    assert_eq!(Path::new(&loaded.repository_root), repo.root);
    assert_eq!(Path::new(&loaded.working_directory), repo.api);
}

#[test]
fn a_proposal_in_a_sub_root_runs_in_that_directory() {
    let repo = Monorepo::new("run-at", "");
    let engine = WorkspaceEngine::new(repo.load());
    let proposal = engine
        .validation_orchestrator
        .propose_command_at(&repo.root, &repo.api, "npm test", "Run the API tests")
        .unwrap();

    let record = engine
        .validation_orchestrator
        .run_proposal(
            &proposal.id,
            true,
            "tester",
            None,
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();

    assert!(
        record.execution.working_directory.ends_with("packages/api"),
        "{}",
        record.execution.working_directory
    );
    assert_eq!(record.execution.exit_code, Some(0));
}

#[test]
fn at_the_repository_root_nothing_changes() {
    let repo = Monorepo::new(
        "unchanged-at-root",
        "command_allowlist=npm test\ncommand_allowlist.<id>=cd packages/api && npm run lint|git push\n",
    );
    let policy = repo.policy();
    let commands = [
        "npm test",
        "npm run lint",
        "git push",
        "git status",
        "ls",
        "cat ../secrets",
        "cat ~/secrets",
        "cat /etc/passwd",
        "cat packages/web/src/index.ts",
        "cd packages/api && npm run lint",
        "rm -rf /",
        "docker ps",
        "make",
    ];
    for command in commands {
        assert_eq!(
            policy.classify_at(command, &repo.root, &repo.root),
            policy.classify(command, &repo.root),
            "{command}"
        );
    }

    // Today's answers, not just agreement between the two entry points.
    let low = |command: &str| {
        let classification = policy.classify(command, &repo.root);
        classification.risk == CommandRisk::Low && !classification.requires_approval
    };
    assert!(low("npm test"));
    assert!(low("git push"));
    assert!(
        !low("npm run lint"),
        "a qualified grant is not a root grant"
    );
    assert!(
        policy
            .classify("cat ../secrets", &repo.root)
            .reasons
            .iter()
            .any(|r| r == OUTSIDE_ROOT_REASON)
    );

    // The existing entry points store the root as both halves of the pair,
    // and Allow Always writes a plain entry.
    let engine = WorkspaceEngine::new(repo.load());
    let orchestrator = &engine.validation_orchestrator;
    assert_eq!(
        orchestrator.classify_command(&repo.root, "npm run lint"),
        orchestrator.classify_command_at(&repo.root, &repo.root, "npm run lint")
    );
    let proposal = orchestrator
        .propose_command(&repo.root, "npm run build", "Build")
        .unwrap();
    assert_eq!(proposal.repository_root, proposal.working_directory);
    orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();
    let written = fs::read_to_string(&repo.user_config).unwrap();
    let root_id = repository_id_for_root(&repo.root);
    let line = written
        .lines()
        .find(|line| line.starts_with(&format!("command_allowlist.{root_id}=")))
        .unwrap();
    assert!(
        line.split('|').any(|entry| entry.trim() == "npm run build"),
        "{line}"
    );
    let reloaded = repo.policy().classify("npm run build", &repo.root);
    assert_eq!(reloaded.risk, CommandRisk::Low);
    assert!(!reloaded.requires_approval);
}
