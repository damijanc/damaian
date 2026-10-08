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
//!
//! Task 3 pins `RepositoryMap::build` over a real index of a temp monorepo:
//! per-root metadata and commands, determinism, `root_for_path` and the
//! input fingerprint.
//!
//! Task 4 pins the user's `project_roots_added` and `project_roots_removed`
//! overrides: what they do to the map, what is rejected, and that both are
//! preference keys.
//!
//! Task 5 pins the persisted map: when a stored file is reused and when it
//! is rebuilt, and why. It also pins validations proposed per root, each at
//! that root's directory.
//!
//! Task 6 pins the map as model context: the rendering stays under
//! `repository_map_max_tokens`, degrades in proposal §5.4's order and says
//! what it dropped, and `build_context` places it before retrieved files.
//! The key is lower-wins at repository scope, and a map that cannot be
//! loaded is left out rather than failing the turn.
//!
//! Task 7 pins the chat loop: a `run_command` may name a repository-relative
//! working directory, an invalid one is refused before anything is stored,
//! findings resolve cwd-first then up the ancestors, and the resume path
//! re-classifies a proposal at its own directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use workspace_engine::finding::resolve_finding_path;
use workspace_engine::indexer::SkippedFile;
use workspace_engine::{
    AuditLog, CancelToken, ChatTurnResult, CommandPolicy, CommandRisk, CommandStore, Config,
    ConfigOverlay, Degradation, DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH,
    MapLoad, MockModelAdapter, ModelAdapter, PROJECT_MANIFESTS, ProjectIndexer, ProjectRoot,
    REPOSITORY_MAP_SCHEMA_VERSION, RebuildReason, RepositoryIndex, RepositoryKeyClass,
    RepositoryMap, RepositoryMapStore, RootCommand, RootDetection, RootEvidence, RootOverride,
    SecretScanner, SessionMode, ToolCall, TurnProgress, TurnSink, WorkspaceEngine, detect_roots,
    repository_id_for_root, split_profile_keys,
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

/// A Cargo workspace with two member crates beside two npm packages, nested
/// `AGENTS.md` files, a `packages/apiary` directory that only shares a
/// prefix with a root, and two ignored build-output directories. Its data
/// directory is outside the repository, so nothing it writes is indexed.
struct IndexedMonorepo {
    root: PathBuf,
    data_dir: PathBuf,
}

const INDEXED_MONOREPO: &[(&str, &str)] = &[
    ("AGENTS.md", "Repository instructions.\n"),
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    ("README.md", "# Monorepo\n"),
    ("crates/AGENTS.md", "Crate instructions.\n"),
    ("crates/cli/Cargo.toml", "[package]\nname = \"cli\"\n"),
    ("crates/cli/src/main.rs", "fn main() {}\n"),
    ("crates/engine/AGENTS.md", "Engine instructions.\n"),
    ("crates/engine/Cargo.toml", "[package]\nname = \"engine\"\n"),
    ("crates/engine/src/lib.rs", "pub fn engine() {}\n"),
    ("crates/engine/tests/it.rs", "#[test]\nfn it() {}\n"),
    ("docs/guide.md", "# Guide\n"),
    (
        "packages/api/package.json",
        "{\"scripts\":{\"test\":\"node test.js\",\"lint\":\"eslint .\"}}",
    ),
    ("packages/api/src/index.ts", "export {};\n"),
    ("packages/api/tests/api.test.ts", "export {};\n"),
    ("packages/api/dist/bundle.js", "generated\n"),
    ("packages/apiary/x.ts", "export {};\n"),
    (
        "packages/web/package.json",
        "{\"scripts\":{\"build\":\"tsc\"}}",
    ),
    ("packages/web/index.js", "module.exports = {};\n"),
    ("target/debug/out.txt", "generated\n"),
];

impl IndexedMonorepo {
    fn new(name: &str) -> Self {
        let root = temp_dir(name);
        for (path, content) in INDEXED_MONOREPO {
            let file = root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        Self {
            root,
            data_dir: temp_dir(&format!("{name}-data")),
        }
    }

    fn config(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            shell: "/usr/bin/true".to_string(),
            ..Config::default()
        }
    }

    fn index(&self) -> RepositoryIndex {
        let config = self.config();
        let scanner = SecretScanner::new(config.secret_patterns.clone());
        let audit_log = AuditLog::new(&self.data_dir, true, scanner.clone());
        ProjectIndexer::new(config, scanner, audit_log)
            .index_repository(&self.root)
            .expect("the fixture should index")
    }

    fn build_with(&self, policy: &CommandPolicy) -> RepositoryMap {
        RepositoryMap::build(&self.index(), policy)
    }

    fn build(&self) -> RepositoryMap {
        self.build_with(&CommandPolicy::new(self.config()))
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    /// The config with these override lists, as `load_scoped` would leave
    /// them after reading `project_roots_added` and `project_roots_removed`.
    fn overriding(&self, added: &[&str], removed: &[&str]) -> Config {
        Config {
            project_roots_added: strings(added),
            project_roots_removed: strings(removed),
            ..self.config()
        }
    }

    fn build_overriding(&self, added: &[&str], removed: &[&str]) -> RepositoryMap {
        self.build_with(&CommandPolicy::new(self.overriding(added, removed)))
    }
}

impl Drop for IndexedMonorepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn command(name: &str, command: &str, risk: CommandRisk, working_directory: &str) -> RootCommand {
    RootCommand {
        name: name.to_string(),
        command: command.to_string(),
        risk,
        working_directory: working_directory.to_string(),
    }
}

fn map_root<'a>(map: &'a RepositoryMap, path: &str) -> &'a ProjectRoot {
    map.roots
        .iter()
        .find(|root| root.path == path)
        .unwrap_or_else(|| panic!("no root {path:?} in {:?}", map.roots))
}

fn without_generated_at(map: &RepositoryMap) -> serde_json::Value {
    let mut value = serde_json::to_value(map).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("generatedAtMs")
        .expect("the map serialises generatedAtMs");
    value
}

#[test]
fn two_builds_of_an_unchanged_repository_serialise_identically() {
    let repo = IndexedMonorepo::new("determinism");
    let first = repo.build();
    let second = repo.build();
    assert_eq!(without_generated_at(&first), without_generated_at(&second));
    assert_eq!(first.schema_version, REPOSITORY_MAP_SCHEMA_VERSION);
    assert_eq!(first.repository_id, repo.index().repository_id);

    // The watcher appends patched records at the end of `files`
    // (context.md §3), so the map must not depend on the index's order.
    let mut index = repo.index();
    index.files.reverse();
    index.skipped.reverse();
    let reordered = RepositoryMap::build(&index, &CommandPolicy::new(repo.config()));
    assert_eq!(
        without_generated_at(&first),
        without_generated_at(&reordered)
    );
}

#[test]
fn nested_roots_each_carry_their_own_metadata_and_commands() {
    let repo = IndexedMonorepo::new("nested");
    let map = repo.build();
    use CommandRisk::Medium;

    let paths: Vec<&str> = map.roots.iter().map(|root| root.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "",
            "crates/cli",
            "crates/engine",
            "packages/api",
            "packages/web"
        ]
    );
    assert_eq!(
        map.excluded,
        vec![
            excluded("packages/api/dist", ExclusionReason::Vendor),
            excluded("target", ExclusionReason::Vendor),
        ]
    );

    assert_eq!(
        *map_root(&map, ""),
        ProjectRoot {
            path: String::new(),
            detected_by: RootEvidence::Manifest {
                path: "Cargo.toml".to_string()
            },
            languages: strings(&["markdown", "toml", "typescript"]),
            manifests: strings(&["Cargo.toml"]),
            entry_points: vec![],
            test_paths: vec![],
            generated_paths: strings(&["target"]),
            major_directories: strings(&["crates", "docs", "packages"]),
            instruction_files: strings(&["AGENTS.md"]),
            commands: vec![command("Cargo.toml", "cargo test", Medium, "")],
            user_override: None,
        }
    );
    assert_eq!(
        *map_root(&map, "crates/engine"),
        ProjectRoot {
            path: "crates/engine".to_string(),
            detected_by: RootEvidence::Manifest {
                path: "crates/engine/Cargo.toml".to_string()
            },
            // Met in walk order as markdown, toml, rust: the map sorts.
            languages: strings(&["markdown", "rust", "toml"]),
            manifests: strings(&["crates/engine/Cargo.toml"]),
            entry_points: strings(&["crates/engine/src/lib.rs"]),
            test_paths: strings(&["crates/engine/tests"]),
            generated_paths: vec![],
            major_directories: strings(&["crates/engine/src", "crates/engine/tests"]),
            instruction_files: strings(&[
                "AGENTS.md",
                "crates/AGENTS.md",
                "crates/engine/AGENTS.md"
            ]),
            commands: vec![command("Cargo.toml", "cargo test", Medium, "crates/engine")],
            user_override: None,
        }
    );
    assert_eq!(
        *map_root(&map, "packages/api"),
        ProjectRoot {
            path: "packages/api".to_string(),
            detected_by: RootEvidence::Manifest {
                path: "packages/api/package.json".to_string()
            },
            languages: strings(&["json", "typescript"]),
            manifests: strings(&["packages/api/package.json"]),
            entry_points: strings(&["packages/api/src/index.ts"]),
            test_paths: strings(&["packages/api/tests"]),
            generated_paths: strings(&["packages/api/dist"]),
            major_directories: strings(&["packages/api/src", "packages/api/tests"]),
            instruction_files: strings(&["AGENTS.md"]),
            commands: vec![
                command("lint", "npm run lint", Medium, "packages/api"),
                command("test", "npm run test", Medium, "packages/api"),
                command("test-shortcut", "npm test", Medium, "packages/api"),
            ],
            user_override: None,
        }
    );

    let cli = map_root(&map, "crates/cli");
    assert_eq!(cli.entry_points, strings(&["crates/cli/src/main.rs"]));
    assert_eq!(
        cli.commands,
        vec![command("Cargo.toml", "cargo test", Medium, "crates/cli")]
    );
    let web = map_root(&map, "packages/web");
    assert_eq!(web.entry_points, strings(&["packages/web/index.js"]));
    assert_eq!(
        web.commands,
        vec![command("build", "npm run build", Medium, "packages/web")]
    );

    assert_eq!(
        serde_json::to_value(command("test", "npm test", Medium, "packages/api")).unwrap(),
        serde_json::json!({
            "name": "test",
            "command": "npm test",
            "risk": "medium",
            "workingDirectory": "packages/api"
        })
    );
}

#[test]
fn a_roots_command_risk_honours_a_grant_qualified_with_that_root() {
    let repo = IndexedMonorepo::new("qualified-risk");
    let mut config = repo.config();
    config
        .command_allowlist
        .push("cd packages/api && npm test".to_string());
    let map = repo.build_with(&CommandPolicy::new(config));

    let risk_of = |root: &str, name: &str| {
        map_root(&map, root)
            .commands
            .iter()
            .find(|command| command.name == name)
            .unwrap_or_else(|| panic!("{root} has no {name} command"))
            .risk
            .clone()
    };
    assert_eq!(risk_of("packages/api", "test-shortcut"), CommandRisk::Low);
    assert_eq!(risk_of("packages/api", "test"), CommandRisk::Medium);
}

#[test]
fn instruction_files_resolve_per_root_broadest_first() {
    let repo = IndexedMonorepo::new("instructions");
    let map = repo.build();
    // Spec 11's ancestor walk for a path inside each root, kept when indexed.
    assert_eq!(
        map_root(&map, "crates/engine").instruction_files,
        strings(&["AGENTS.md", "crates/AGENTS.md", "crates/engine/AGENTS.md"])
    );
    assert_eq!(
        map_root(&map, "crates/cli").instruction_files,
        strings(&["AGENTS.md", "crates/AGENTS.md"])
    );
    assert_eq!(
        map_root(&map, "packages/web").instruction_files,
        strings(&["AGENTS.md"])
    );
}

#[test]
fn root_for_path_is_the_longest_root_that_is_a_segment_prefix() {
    let repo = IndexedMonorepo::new("root-for-path");
    let map = repo.build();
    assert_eq!(
        map.root_for_path("packages/api/src/index.ts"),
        "packages/api"
    );
    assert_eq!(map.root_for_path("packages/api"), "packages/api");
    assert_eq!(map.root_for_path("packages/apiary/x.ts"), "");
    assert_eq!(map.root_for_path("README.md"), "");
    assert_eq!(
        map.root_for_path("crates/engine/src/lib.rs"),
        "crates/engine"
    );
    assert_eq!(map.root_for_path("crates/AGENTS.md"), "");
}

#[test]
fn the_fingerprint_tracks_manifest_content_and_not_modification_times() {
    let repo = IndexedMonorepo::new("fingerprint");
    let before = repo.build().fingerprint;

    let readme = fs::File::options()
        .write(true)
        .open(repo.root.join("README.md"))
        .unwrap();
    readme
        .set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    drop(readme);
    assert_eq!(repo.build().fingerprint, before, "an mtime is not an input");

    fs::write(
        repo.root.join("packages/api/package.json"),
        "{\"scripts\":{\"test\":\"node test.js\",\"lint\":\"biome lint\"}}",
    )
    .unwrap();
    assert_ne!(
        repo.build().fingerprint,
        before,
        "a manifest's content is an input"
    );
}

#[test]
fn detect_project_commands_is_unchanged() {
    let dir = temp_dir("detect-unchanged");
    fs::write(
        dir.join("package.json"),
        "{\"scripts\":{\"format\":\"x\",\"test\":\"x\",\"lint\":\"x\"}}",
    )
    .unwrap();
    fs::write(dir.join("Cargo.toml"), "").unwrap();
    fs::write(dir.join("go.mod"), "").unwrap();
    let policy = CommandPolicy::new(Config {
        data_dir: dir.join(".damaian"),
        enable_index_watcher: false,
        command_allowlist: vec!["npm test".to_string()],
        ..Config::default()
    });
    let commands: Vec<(String, String, CommandRisk)> = policy
        .detect_project_commands(&dir)
        .unwrap()
        .into_iter()
        .map(|command| (command.name, command.command, command.risk))
        .collect();
    let _ = fs::remove_dir_all(&dir);

    // Script order, then the shortcut, then the manifest table, unsorted,
    // classified at the root so a plain grant applies.
    let expected = [
        ("test", "npm run test", CommandRisk::Medium),
        ("lint", "npm run lint", CommandRisk::Medium),
        ("format", "npm run format", CommandRisk::Medium),
        ("test-shortcut", "npm test", CommandRisk::Low),
        ("go.mod", "go test ./...", CommandRisk::Medium),
        ("Cargo.toml", "cargo test", CommandRisk::Medium),
    ]
    .map(|(name, command, risk)| (name.to_string(), command.to_string(), risk));
    assert_eq!(commands, expected);
}

// Task 4: user overrides (context.md §6). Both lists are preference keys,
// validated when the map is built, and a rejected entry is recorded in
// `excluded` rather than applied.

#[test]
fn an_added_override_makes_a_root_the_user_named() {
    let repo = IndexedMonorepo::new("override-added");
    repo.write("tools/scripts/release.sh", "echo release\n");
    assert_eq!(repo.build().root_for_path("tools/scripts/release.sh"), "");

    let map = repo.build_overriding(&["tools/scripts"], &[]);
    let added = map_root(&map, "tools/scripts");
    assert_eq!(added.detected_by, RootEvidence::UserOverride);
    assert_eq!(added.user_override, Some(RootOverride::Added));
    let json = serde_json::to_value(added).unwrap();
    assert_eq!(json["detectedBy"]["kind"], "userOverride");
    assert_eq!(json["userOverride"], "added");
    assert_eq!(
        map.root_for_path("tools/scripts/release.sh"),
        "tools/scripts"
    );
    assert!(
        map.excluded
            .iter()
            .all(|entry| entry.reason != ExclusionReason::InvalidOverride),
        "{:?}",
        map.excluded
    );

    // A root detection also found keeps its manifest as the evidence.
    let map = repo.build_overriding(&["packages/api"], &[]);
    let api = map_root(&map, "packages/api");
    assert_eq!(
        api.detected_by,
        RootEvidence::Manifest {
            path: "packages/api/package.json".to_string()
        }
    );
    assert_eq!(api.user_override, Some(RootOverride::Added));
    assert_eq!(api.commands.len(), 3);
}

#[test]
fn a_removed_root_stays_visible_and_its_files_go_to_the_enclosing_root() {
    let repo = IndexedMonorepo::new("override-removed");
    repo.write(
        "examples/package.json",
        "{\"scripts\":{\"test\":\"node t.js\"}}",
    );
    repo.write(
        "examples/legacy/package.json",
        "{\"scripts\":{\"test\":\"node t.js\"}}",
    );
    repo.write("examples/legacy/index.js", "module.exports = {};\n");

    let detected = repo.build();
    assert!(!map_root(&detected, "examples/legacy").commands.is_empty());
    assert_eq!(
        detected.root_for_path("examples/legacy/index.js"),
        "examples/legacy"
    );
    assert!(map_root(&detected, "examples").major_directories.is_empty());

    let map = repo.build_overriding(&[], &["examples/legacy"]);
    // Kept, with its evidence, so the UI can show the removal and undo it.
    assert_eq!(
        *map_root(&map, "examples/legacy"),
        ProjectRoot {
            path: "examples/legacy".to_string(),
            detected_by: RootEvidence::Manifest {
                path: "examples/legacy/package.json".to_string()
            },
            languages: vec![],
            manifests: vec![],
            entry_points: vec![],
            test_paths: vec![],
            generated_paths: vec![],
            major_directories: vec![],
            instruction_files: vec![],
            commands: vec![],
            user_override: Some(RootOverride::Removed),
        }
    );
    assert_eq!(
        serde_json::to_value(map_root(&map, "examples/legacy")).unwrap()["userOverride"],
        "removed"
    );
    // No longer a root for anything else: its files, languages and directory
    // belong to the nearest root that is not removed.
    assert_eq!(map.root_for_path("examples/legacy/index.js"), "examples");
    let examples = map_root(&map, "examples");
    assert_eq!(examples.major_directories, strings(&["examples/legacy"]));
    assert!(examples.languages.contains(&"javascript".to_string()));
    assert!(
        examples
            .manifests
            .iter()
            .all(|manifest| manifest == "examples/package.json"),
        "{:?}",
        examples.manifests
    );
}

#[test]
fn overrides_survive_a_rebuild_after_the_repository_changed() {
    let repo = IndexedMonorepo::new("override-rebuild");
    repo.write("tools/scripts/release.sh", "echo release\n");
    repo.write(
        "examples/legacy/package.json",
        "{\"scripts\":{\"test\":\"node t.js\"}}",
    );
    let policy = CommandPolicy::new(repo.overriding(&["tools/scripts"], &["examples/legacy"]));
    let before = repo.build_with(&policy);

    repo.write("tools/scripts/notes.md", "# Notes\n");
    repo.write(
        "examples/legacy/package.json",
        "{\"scripts\":{\"test\":\"node t.js\",\"lint\":\"eslint .\"}}",
    );
    // `build_with` indexes afresh, so this is a rescan of a changed tree.
    let after = repo.build_with(&policy);
    assert_ne!(before.fingerprint, after.fingerprint);

    for map in [&before, &after] {
        let added = map_root(map, "tools/scripts");
        assert_eq!(added.detected_by, RootEvidence::UserOverride);
        assert_eq!(added.user_override, Some(RootOverride::Added));
        let removed = map_root(map, "examples/legacy");
        assert_eq!(removed.user_override, Some(RootOverride::Removed));
        assert!(removed.commands.is_empty());
    }
    assert!(
        map_root(&after, "tools/scripts")
            .languages
            .contains(&"markdown".to_string())
    );
}

#[test]
fn the_fingerprint_includes_the_override_lists() {
    let repo = IndexedMonorepo::new("override-fingerprint");
    let plain = repo.build().fingerprint;
    let added = repo.build_overriding(&["docs"], &[]).fingerprint;
    let removed = repo.build_overriding(&[], &["packages/web"]).fingerprint;
    assert_ne!(plain, added);
    assert_ne!(plain, removed);
    assert_ne!(added, removed);
    assert_eq!(repo.build_overriding(&["docs"], &[]).fingerprint, added);
}

#[test]
fn an_invalid_override_is_recorded_and_changes_nothing_else() {
    let repo = IndexedMonorepo::new("override-invalid");
    fs::create_dir_all(repo.root.join("empty")).unwrap();
    repo.write("node_modules/x/index.js", "module.exports = {};\n");
    repo.write(".env/local.txt", "placeholder\n");
    // Indexed, so only `restricted_patterns` can reject `.env`.
    assert!(
        repo.index()
            .files
            .iter()
            .any(|file| file.path == ".env/local.txt")
    );
    let baseline = repo.build();

    let cases: [(&[&str], &[&str], &str); 13] = [
        (&["../outside"], &[], "../outside"),
        (&["/etc"], &[], "/etc"),
        (&["node_modules/x"], &[], "node_modules/x"),
        (&["empty"], &[], "empty"),
        (&[".env"], &[], ".env"),
        (&["./docs"], &[], "./docs"),
        (&["docs/"], &[], "docs/"),
        (&["packages//api"], &[], "packages//api"),
        // A file, not a directory holding one.
        (&["README.md"], &[], "README.md"),
        // The repository root is never removed: the map is never empty.
        (&[], &[""], ""),
        (&[], &["."], "."),
        // Names no root, so removing it would do nothing.
        (&[], &["docs"], "docs"),
        // Both added and removed: neither applies.
        (&["docs"], &["docs"], "docs"),
    ];
    for (added, removed, entry) in cases {
        let map = repo.build_overriding(added, removed);
        assert_eq!(map.roots, baseline.roots, "{entry:?} changed the roots");
        let mut expected = baseline.excluded.clone();
        expected.push(excluded(entry, ExclusionReason::InvalidOverride));
        expected.sort();
        assert_eq!(map.excluded, expected, "{entry:?}");
    }
}

#[test]
fn repository_config_sets_the_override_keys_and_a_profile_cannot_carry_them() {
    let repo = IndexedMonorepo::new("override-config");
    let repository_config = Config::repository_config_path(&repo.root);
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    fs::write(
        &repository_config,
        "project_roots_added=tools/scripts|tools/ci\nproject_roots_removed=examples/legacy\n",
    )
    .unwrap();
    let (config, report) = Config::load_scoped(
        repo.config(),
        None,
        Some(&repository_config),
        None,
        Some(&repo.root),
    )
    .unwrap();
    assert_eq!(
        config.project_roots_added,
        strings(&["tools/scripts", "tools/ci"])
    );
    assert_eq!(config.project_roots_removed, strings(&["examples/legacy"]));
    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    let shown = config.to_policy_text();
    assert!(
        shown.contains("project_roots_added=tools/scripts|tools/ci\n"),
        "{shown}"
    );
    assert!(
        shown.contains("project_roots_removed=examples/legacy\n"),
        "{shown}"
    );

    let overlay =
        ConfigOverlay::parse("project_roots_added=a|b\nproject_roots_removed=c\n").unwrap();
    assert_eq!(
        ConfigOverlay::parse(&overlay.to_policy_text()).unwrap(),
        overlay
    );
    let (carried, refused) = split_profile_keys(overlay);
    assert_eq!(carried, ConfigOverlay::default());
    let refused: Vec<(&str, RepositoryKeyClass)> = refused
        .iter()
        .map(|rejected| (rejected.key.as_str(), rejected.class))
        .collect();
    assert_eq!(
        refused,
        vec![
            ("project_roots_added", RepositoryKeyClass::Forbidden),
            ("project_roots_removed", RepositoryKeyClass::Forbidden),
        ]
    );
}

#[test]
fn editing_the_overrides_keeps_a_path_in_at_most_one_list() {
    use workspace_engine::{RootOverrideEdit, edit_root_overrides};
    let mut overlay = ConfigOverlay::parse("max_file_bytes=2048\n").unwrap();
    edit_root_overrides(&mut overlay, RootOverrideEdit::Add, "tools/scripts");
    edit_root_overrides(&mut overlay, RootOverrideEdit::Add, "tools/scripts");
    edit_root_overrides(&mut overlay, RootOverrideEdit::Remove, "examples/legacy");
    assert_eq!(
        overlay.project_roots_added,
        Some(strings(&["tools/scripts"]))
    );
    assert_eq!(
        overlay.project_roots_removed,
        Some(strings(&["examples/legacy"]))
    );

    edit_root_overrides(&mut overlay, RootOverrideEdit::Remove, "tools/scripts");
    assert_eq!(overlay.project_roots_added, None);
    assert_eq!(
        overlay.project_roots_removed,
        Some(strings(&["examples/legacy", "tools/scripts"]))
    );

    edit_root_overrides(&mut overlay, RootOverrideEdit::Clear, "examples/legacy");
    edit_root_overrides(&mut overlay, RootOverrideEdit::Clear, "tools/scripts");
    // Emptied lists are unset, and the user's other keys are untouched.
    assert_eq!(
        overlay,
        ConfigOverlay::parse("max_file_bytes=2048\n").unwrap()
    );
}

impl IndexedMonorepo {
    fn store(&self) -> RepositoryMapStore {
        let scanner = SecretScanner::new(self.config().secret_patterns.clone());
        RepositoryMapStore::new(&self.data_dir, AuditLog::new(&self.data_dir, true, scanner))
    }

    fn load_with(&self, policy: &CommandPolicy) -> (RepositoryMap, MapLoad) {
        self.store()
            .load_or_build(&self.index(), policy)
            .expect("the map should load or build")
    }

    fn load(&self) -> (RepositoryMap, MapLoad) {
        self.load_with(&CommandPolicy::new(self.config()))
    }

    fn map_file(&self) -> PathBuf {
        RepositoryMapStore::path(&self.data_dir, &self.index().repository_id)
    }

    /// The `repository_map_rebuilt` events in the audit log, oldest first.
    fn rebuild_events(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.data_dir.join("audit/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["eventType"] == "repository_map_rebuilt")
            .collect()
    }
}

#[test]
fn a_missing_map_is_built_and_an_unchanged_repository_reuses_it_untouched() {
    let repo = IndexedMonorepo::new("store-reuse");
    let (built, outcome) = repo.load();
    assert_eq!(outcome, MapLoad::Built);
    let path = repo.map_file();
    assert!(
        path.ends_with(format!("repository-map/{}.json", built.repository_id)),
        "{}",
        path.display()
    );
    let bytes = fs::read(&path).expect("the map is written");
    assert_eq!(
        serde_json::from_slice::<RepositoryMap>(&bytes).unwrap(),
        built
    );

    let (reused, outcome) = repo.load();
    assert_eq!(outcome, MapLoad::Reused);
    assert_eq!(reused, built, "the stored map, generatedAtMs included");
    assert_eq!(fs::read(&path).unwrap(), bytes, "a reuse does not rewrite");
    assert!(repo.rebuild_events().is_empty(), "neither is a rebuild");
    // No temp file is left behind beside the map.
    let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(entries.len(), 1);
}

#[test]
fn a_garbage_map_file_is_rebuilt_as_corrupt_and_audited() {
    let repo = IndexedMonorepo::new("store-corrupt");
    let path = repo.map_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ not json").unwrap();

    let (map, outcome) = repo.load();
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::Corrupt
        }
    );
    assert_eq!(
        serde_json::from_slice::<RepositoryMap>(&fs::read(&path).unwrap()).unwrap(),
        map
    );
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "corrupt");
    assert_eq!(events[0]["repositoryId"], map.repository_id.as_str());
    assert_eq!(repo.load().1, MapLoad::Reused);
}

#[test]
fn a_map_from_another_schema_version_is_rebuilt_as_a_mismatch() {
    let repo = IndexedMonorepo::new("store-schema");
    repo.load();
    let path = repo.map_file();
    let mut stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    stored["schemaVersion"] = 0.into();
    fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();

    let (map, outcome) = repo.load();
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::SchemaMismatch { found: 0 }
        }
    );
    assert_eq!(map.schema_version, REPOSITORY_MAP_SCHEMA_VERSION);
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "schemaMismatch");
    assert_eq!(events[0]["foundSchemaVersion"], "0");
}

#[test]
fn a_manifest_change_rebuilds_the_map_as_stale() {
    let repo = IndexedMonorepo::new("store-stale");
    let (before, _) = repo.load();
    assert_eq!(map_root(&before, "packages/web").commands.len(), 1);

    repo.write(
        "packages/web/package.json",
        "{\"scripts\":{\"build\":\"tsc\",\"lint\":\"eslint .\"}}",
    );
    let (after, outcome) = repo.load();
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::Stale
        }
    );
    assert_eq!(map_root(&after, "packages/web").commands.len(), 2);
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "stale");
}

/// Task 4's gap: command risk depends on `command_allowlist`, so a grant
/// made after the map was stored must not leave the stored risk in place.
#[test]
fn a_new_allowlist_grant_makes_the_stored_map_stale() {
    let repo = IndexedMonorepo::new("store-allowlist");
    let risk_of = |map: &RepositoryMap| {
        map_root(map, "packages/api")
            .commands
            .iter()
            .find(|command| command.name == "test-shortcut")
            .expect("packages/api has npm test")
            .risk
            .clone()
    };
    let (before, _) = repo.load();
    assert_eq!(risk_of(&before), CommandRisk::Medium);

    let mut config = repo.config();
    config
        .command_allowlist
        .push("cd packages/api && npm test".to_string());
    let (after, outcome) = repo.load_with(&CommandPolicy::new(config.clone()));
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::Stale
        }
    );
    assert_eq!(risk_of(&after), CommandRisk::Low);

    // A blocklist entry changes risk too.
    config.command_blocklist.push("npm test".to_string());
    let (blocked, outcome) = repo.load_with(&CommandPolicy::new(config));
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::Stale
        }
    );
    assert_eq!(risk_of(&blocked), CommandRisk::Blocked);
}

/// `restricted_patterns` decides whether an override applies. It reaches the
/// fingerprint through the `invalidOverride` entry it causes, not by being
/// hashed itself.
#[test]
fn a_new_restricted_pattern_makes_the_stored_map_stale() {
    let repo = IndexedMonorepo::new("store-restricted");
    let config = repo.overriding(&["docs"], &[]);
    let (before, _) = repo.load_with(&CommandPolicy::new(config.clone()));
    assert_eq!(
        map_root(&before, "docs").user_override,
        Some(RootOverride::Added)
    );

    let mut restricted = config;
    restricted.restricted_patterns.push("docs".to_string());
    let (after, outcome) = repo.load_with(&CommandPolicy::new(restricted));
    assert_eq!(
        outcome,
        MapLoad::Rebuilt {
            reason: RebuildReason::Stale
        }
    );
    assert!(after.roots.iter().all(|root| root.path != "docs"));
    assert!(
        after
            .excluded
            .contains(&excluded("docs", ExclusionReason::InvalidOverride))
    );
}

#[test]
fn the_engine_builds_the_map_once_and_then_reuses_it() {
    let repo = IndexedMonorepo::new("engine-map");
    let engine = WorkspaceEngine::new(repo.config());
    let (built, outcome) = engine.repository_map(&repo.root).unwrap();
    assert_eq!(outcome, MapLoad::Built);
    assert_eq!(built.repository_id, repo.index().repository_id);
    let (reused, outcome) = engine.repository_map(&repo.root).unwrap();
    assert_eq!(outcome, MapLoad::Reused);
    assert_eq!(reused, built);
}

#[test]
fn detected_validations_are_proposed_and_run_at_their_own_root() {
    let repo = IndexedMonorepo::new("validations-per-root");
    let engine = WorkspaceEngine::new(repo.config());
    let (map, _) = engine.repository_map(&repo.root).unwrap();
    let proposals = engine
        .validation_orchestrator
        .propose_detected_validations(&repo.root, &map)
        .unwrap();

    let repository_root = repo.root.to_string_lossy().to_string();
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.repository_root == repository_root),
        "{proposals:?}"
    );
    // At the repository root nothing changes: its commands run there.
    let at_root = proposals
        .iter()
        .find(|proposal| proposal.command == "cargo test")
        .expect("the workspace root proposes cargo test");
    assert_eq!(at_root.working_directory, repository_root);

    let api = proposals
        .iter()
        .find(|proposal| proposal.command == "npm run lint")
        .expect("packages/api proposes its lint script");
    assert!(
        api.working_directory.ends_with("packages/api"),
        "{}",
        api.working_directory
    );
    let web: Vec<_> = proposals
        .iter()
        .filter(|proposal| proposal.working_directory.ends_with("packages/web"))
        .map(|proposal| proposal.command.as_str())
        .collect();
    assert_eq!(web, ["npm run build"]);

    let record = engine
        .validation_orchestrator
        .run_proposal(
            &api.id,
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
fn a_removed_root_yields_no_proposal() {
    let repo = IndexedMonorepo::new("validations-removed");
    let engine = WorkspaceEngine::new(repo.config());
    let in_web = |map: &RepositoryMap| {
        engine
            .validation_orchestrator
            .propose_detected_validations(&repo.root, map)
            .unwrap()
            .into_iter()
            .filter(|proposal| proposal.working_directory.ends_with("packages/web"))
            .count()
    };

    let removed = repo.build_overriding(&[], &["packages/web"]);
    assert_eq!(in_web(&removed), 0);

    // The field decides, not the empty command list: a map that still lists
    // a removed root's commands proposes none of them.
    let mut marked = repo.build();
    assert_eq!(in_web(&marked), 1);
    marked
        .roots
        .iter_mut()
        .find(|root| root.path == "packages/web")
        .unwrap()
        .user_override = Some(RootOverride::Removed);
    assert_eq!(in_web(&marked), 0);
}

// Task 6: bounded rendering and the `repository_map` context item.

fn project_root(path: &str, entry_points: &[&str], test_paths: &[&str]) -> ProjectRoot {
    let under = |child: &str| {
        if path.is_empty() {
            child.to_string()
        } else {
            format!("{path}/{child}")
        }
    };
    ProjectRoot {
        path: path.to_string(),
        detected_by: RootEvidence::Manifest {
            path: under("package.json"),
        },
        languages: strings(&["json", "typescript"]),
        manifests: vec![under("package.json")],
        entry_points: strings(entry_points),
        test_paths: strings(test_paths),
        generated_paths: vec![under("dist")],
        major_directories: vec![under("src"), under("tests")],
        instruction_files: Vec::new(),
        commands: vec![command("test", "npm run test", CommandRisk::Medium, path)],
        user_override: None,
    }
}

fn synthetic_map(roots: Vec<ProjectRoot>) -> RepositoryMap {
    RepositoryMap {
        repository_id: "repo_sha256:000000000".to_string(),
        schema_version: REPOSITORY_MAP_SCHEMA_VERSION,
        generated_at_ms: 0,
        fingerprint: String::new(),
        roots,
        excluded: Vec::new(),
    }
}

/// Three roots whose entry-point and test lines differ in size, so the
/// largest is unambiguous, beside a removed root that must never render.
fn three_root_map() -> RepositoryMap {
    let mut removed = project_root("examples/legacy", &[], &[]);
    removed.user_override = Some(RootOverride::Removed);
    synthetic_map(vec![
        project_root("", &["src/main.ts"], &["tests"]),
        removed,
        project_root(
            "packages/api",
            &[
                "packages/api/src/index.ts",
                "packages/api/src/main.ts",
                "packages/api/index.ts",
            ],
            &["packages/api/tests"],
        ),
        project_root("packages/api/v2", &["packages/api/v2/index.ts"], &[]),
    ])
}

fn label(degradation: &Degradation) -> String {
    match degradation {
        Degradation::EntryPointsAndTestPaths { root } => format!("details:{root}"),
        Degradation::MajorDirectories => "directories".to_string(),
        Degradation::GeneratedPathsCounted => "generated".to_string(),
        Degradation::Roots { omitted } => format!("roots:{}", omitted.join(",")),
    }
}

#[test]
fn a_small_map_is_rendered_whole_with_no_degradation_line() {
    let map = three_root_map();
    let rendered = map.render_for_model(800);
    assert!(rendered.dropped.is_empty(), "{:?}", rendered.dropped);
    assert_eq!(rendered.roots_total, 3, "a removed root is not counted");
    assert_eq!(rendered.roots_shown, 3);
    assert_eq!(rendered.tokens, rendered.text.len().div_ceil(4));
    for expected in [
        "packages/api/src/main.ts",
        "packages/api/tests",
        "packages/api/v2/dist",
        "npm run test",
        "typescript",
    ] {
        assert!(
            rendered.text.contains(expected),
            "{expected}: {}",
            rendered.text
        );
    }
    assert!(
        !rendered.text.contains("examples/legacy"),
        "{}",
        rendered.text
    );
    assert!(!rendered.text.contains("omitted"), "{}", rendered.text);
    assert!(!rendered.text.contains("shown"), "{}", rendered.text);
}

#[test]
fn each_degradation_step_fires_in_order_as_the_ceiling_falls() {
    let map = three_root_map();
    let full = map.render_for_model(usize::MAX);
    assert!(full.dropped.is_empty());

    let mut previous: Vec<String> = Vec::new();
    let mut last_fitting = Vec::new();
    for ceiling in (0..=full.tokens).rev() {
        let rendered = map.render_for_model(ceiling);
        assert!(
            rendered.tokens <= ceiling,
            "{} tokens at a ceiling of {ceiling}: {}",
            rendered.tokens,
            rendered.text
        );
        let labels: Vec<String> = rendered.dropped.iter().map(label).collect();
        // A lower ceiling only ever drops more: each step extends the
        // previous one, or omits a further root.
        let extends = labels.starts_with(&previous)
            || (labels.len() == previous.len()
                && labels[..labels.len() - 1] == previous[..previous.len() - 1]
                && labels.last().unwrap().starts_with("roots:")
                && labels.last().unwrap().starts_with(previous.last().unwrap()));
        assert!(extends, "{previous:?} then {labels:?} at {ceiling}");
        if !labels.is_empty() {
            assert!(
                rendered.text.is_empty() || rendered.text.contains("roots shown"),
                "a degraded map says so: {}",
                rendered.text
            );
        }
        if rendered.roots_shown > 0 {
            last_fitting = labels.clone();
        }
        previous = labels;
    }

    assert_eq!(
        last_fitting,
        vec![
            // Largest first, by the length of what is dropped.
            "details:packages/api".to_string(),
            "details:packages/api/v2".to_string(),
            "details:".to_string(),
            "directories".to_string(),
            "generated".to_string(),
            // Deepest first, and the repository root is never dropped.
            "roots:packages/api/v2,packages/api".to_string(),
        ]
    );
}

#[test]
fn sixty_long_roots_stay_under_the_default_ceiling_and_say_what_was_dropped() {
    let mut roots = vec![project_root("", &["src/main.ts"], &["tests"])];
    for group in 0..6 {
        for package in 0..10 {
            let path = format!(
                "packages/group-{group:02}/a-rather-long-package-directory-name-{package:02}"
            );
            let entry = format!("{path}/src/index.ts");
            let tests = format!("{path}/tests");
            roots.push(project_root(&path, &[&entry], &[&tests]));
        }
    }
    let map = synthetic_map(roots);
    let rendered = map.render_for_model(800);
    assert!(rendered.tokens <= 800, "{} tokens", rendered.tokens);
    assert!(rendered.text.len().div_ceil(4) <= 800);
    assert_eq!(rendered.roots_total, 61);
    assert!(rendered.roots_shown < 61, "{}", rendered.roots_shown);
    assert!(rendered.roots_shown >= 1);
    assert!(
        rendered
            .text
            .contains(&format!("{} of 61 roots shown", rendered.roots_shown)),
        "{}",
        rendered.text
    );
    assert!(
        rendered
            .text
            .contains("entry points and test paths omitted"),
        "{}",
        rendered.text
    );
    let Some(Degradation::Roots { omitted }) = rendered.dropped.last() else {
        panic!("roots were dropped last: {:?}", rendered.dropped);
    };
    assert_eq!(omitted.len(), 61 - rendered.roots_shown);
    assert!(
        !omitted.contains(&String::new()),
        "the repository root stays"
    );
    // Equal depth, so by path: the last package of the last group goes first.
    assert_eq!(
        omitted[0],
        "packages/group-05/a-rather-long-package-directory-name-09"
    );
}

#[test]
fn build_context_places_one_map_item_before_retrieved_files() {
    let repo = IndexedMonorepo::new("context-item");
    let engine = WorkspaceEngine::new(repo.config());
    let index = repo.index();
    let plan = engine.context_manager.build_context(
        &repo.root,
        &index.repository_id,
        "task_map",
        "change the engine function and the api index",
        Some(&index),
        &[],
        16_000,
    );
    let kinds: Vec<&str> = plan.items.iter().map(|item| item.kind.as_str()).collect();
    let maps: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == "repository_map")
        .map(|(position, _)| position)
        .collect();
    assert_eq!(maps.len(), 1, "{kinds:?}");
    let first_retrieved = kinds
        .iter()
        .position(|kind| *kind == "retrieved_file")
        .expect("the prompt retrieves files, or this test proves nothing");
    assert!(maps[0] < first_retrieved, "{kinds:?}");
    let last_rule = kinds.iter().rposition(|kind| *kind == "project_rule");
    assert!(last_rule.is_none_or(|rule| rule < maps[0]), "{kinds:?}");

    let item = &plan.items[maps[0]];
    assert_eq!(item.path, None);
    assert!(item.tokens <= 800);
    assert!(item.content.contains("packages/api"), "{}", item.content);
    assert!(
        !plan
            .files
            .iter()
            .any(|file| file.contains("repository-map")),
        "{:?}",
        plan.files
    );

    let off = WorkspaceEngine::new(Config {
        repository_map_max_tokens: 0,
        ..repo.config()
    });
    let plan = off.context_manager.build_context(
        &repo.root,
        &index.repository_id,
        "task_map_off",
        "change the engine function and the api index",
        Some(&index),
        &[],
        16_000,
    );
    assert!(
        !plan.items.iter().any(|item| item.kind == "repository_map"),
        "a ceiling of 0 turns the item off"
    );

    let plan = engine.context_manager.build_context(
        &repo.root,
        &index.repository_id,
        "task_map_no_index",
        "change the engine function",
        None,
        &[],
        16_000,
    );
    assert!(!plan.items.iter().any(|item| item.kind == "repository_map"));
}

#[test]
fn a_map_that_cannot_be_stored_is_left_out_and_the_turn_goes_on() {
    let repo = IndexedMonorepo::new("context-item-unwritable");
    // A file where the map directory should be, so the store cannot write.
    fs::write(repo.data_dir.join("repository-map"), "not a directory").unwrap();
    let engine = WorkspaceEngine::new(repo.config());
    assert!(
        engine.repository_map(&repo.root).is_err(),
        "the store fails"
    );

    let index = repo.index();
    let plan = engine.context_manager.build_context(
        &repo.root,
        &index.repository_id,
        "task_map_unwritable",
        "change the engine function and the api index",
        Some(&index),
        &[],
        16_000,
    );
    let kinds: Vec<&str> = plan.items.iter().map(|item| item.kind.as_str()).collect();
    assert!(!kinds.contains(&"repository_map"), "{kinds:?}");
    assert!(kinds.contains(&"user_prompt"), "{kinds:?}");
    assert!(kinds.contains(&"retrieved_file"), "{kinds:?}");
}

#[test]
fn repository_config_may_lower_the_map_ceiling_but_not_raise_it() {
    let repo = IndexedMonorepo::new("map-ceiling-config");
    assert_eq!(repo.config().repository_map_max_tokens, 800);
    let repository_config = Config::repository_config_path(&repo.root);
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    let load = |line: &str| {
        fs::write(&repository_config, line).unwrap();
        Config::load_scoped(
            repo.config(),
            None,
            Some(&repository_config),
            None,
            Some(&repo.root),
        )
        .unwrap()
    };

    let (config, report) = load("repository_map_max_tokens=100000\n");
    assert_eq!(config.repository_map_max_tokens, 800);
    let refused = report
        .rejected_keys
        .iter()
        .find(|rejected| rejected.key == "repository_map_max_tokens")
        .unwrap_or_else(|| panic!("not refused: {:?}", report.rejected_keys));
    assert_eq!(refused.class, RepositoryKeyClass::RestrictOnly);

    let (config, report) = load("repository_map_max_tokens=50\n");
    assert_eq!(config.repository_map_max_tokens, 50);
    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    assert!(
        config
            .to_policy_text()
            .contains("repository_map_max_tokens=50\n")
    );

    let (config, _) = load("repository_map_max_tokens=0\n");
    assert_eq!(
        config.repository_map_max_tokens, 0,
        "a repository may turn it off"
    );

    let overlay = ConfigOverlay::parse("repository_map_max_tokens=120\n").unwrap();
    assert_eq!(overlay.repository_map_max_tokens, Some(120));
    assert_eq!(
        ConfigOverlay::parse(&overlay.to_policy_text()).unwrap(),
        overlay
    );
    // A capability key: a profile carries it.
    let (carried, refused) = split_profile_keys(overlay.clone());
    assert_eq!(carried, overlay);
    assert!(refused.is_empty(), "{refused:?}");
}

// -- Task 7: commands and findings in a sub-root, in the chat loop --------

fn tool_call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// Scripted tool-call rounds, then a plain answer so the loop ends.
fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn chat_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let cancel = CancelToken::new();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, "Go.", &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

fn approve(engine: &WorkspaceEngine, proposal_id: &str) -> ChatTurnResult {
    let mut after = MockModelAdapter::new("Understood.");
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let cancel = CancelToken::new();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision(proposal_id, true, "tester", &mut after, &mut sink)
        .expect("the resumed turn should run")
}

fn tool_messages(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
    engine
        .session_store
        .read_messages(session_id)
        .expect("messages should read")
        .into_iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content)
        .collect()
}

fn audited(engine: &WorkspaceEngine, event_type: &str) -> usize {
    fs::read_to_string(engine.config.data_dir.join("audit/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["eventType"] == event_type)
        .count()
}

/// A command requested for a sub-root is proposed there, with the repository
/// root kept as the boundary. A request without the argument behaves as it
/// always did.
#[test]
fn a_requested_working_directory_routes_the_proposal_to_that_root() {
    let repo = IndexedMonorepo::new("task7-subroot");
    let engine = WorkspaceEngine::new(repo.config());

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"npm test","reason":"Test it","working_directory":"packages/api"}"#,
    )]]);
    let requested = chat_turn(&engine, &repo.root, &mut adapter);
    let card = requested.command_proposal.expect("npm test needs approval");
    let proposal = engine.command_store.load_proposal(&card.id).unwrap();
    assert!(
        proposal.working_directory.ends_with("packages/api"),
        "{}",
        proposal.working_directory
    );
    assert_eq!(
        proposal.repository_root,
        repo.root.to_string_lossy(),
        "the repository root stays the boundary"
    );

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"npm test","reason":"Test it"}"#,
    )]]);
    let defaulted = chat_turn(&engine, &repo.root, &mut adapter);
    let card = defaulted.command_proposal.expect("npm test needs approval");
    let proposal = engine.command_store.load_proposal(&card.id).unwrap();
    assert_eq!(
        proposal.working_directory, proposal.repository_root,
        "no argument means the repository root, as before"
    );
}

/// A `..`, an absolute path, a symlink out of the repository, a file and a
/// restricted directory are each refused before anything is stored, and the
/// tool result says why.
#[test]
fn a_refused_working_directory_never_stores_a_proposal() {
    let repo = IndexedMonorepo::new("task7-invalid");
    let outside_name = format!("damaian-task7-outside-{}", std::process::id());
    let outside = repo.root.parent().unwrap().join(&outside_name);
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, repo.root.join("escape")).unwrap();
    fs::create_dir_all(repo.root.join(".env")).unwrap();
    let parent_escape = format!("../{outside_name}");

    for (requested, needle) in [
        (parent_escape.as_str(), "outside"),
        ("/etc", "outside"),
        ("escape", "outside"),
        ("packages/api/package.json", "directory"),
        (".env", "restricted"),
    ] {
        let engine = WorkspaceEngine::new(repo.config());
        let arguments = serde_json::json!({
            "command": "ls",
            "reason": "List",
            "working_directory": requested,
        })
        .to_string();
        let mut adapter = scripted(vec![vec![tool_call("run_command", &arguments)]]);
        let result = chat_turn(&engine, &repo.root, &mut adapter);

        assert!(result.command_proposal.is_none(), "{requested}");
        let results = tool_messages(&engine, &result.session.id);
        let last = results.last().unwrap_or_else(|| panic!("{requested}"));
        assert!(
            last.to_lowercase().contains(needle),
            "{requested}: {last:?}"
        );
        assert_eq!(
            audited(&engine, "stored_command_executed"),
            0,
            "{requested} ran"
        );
    }
    let _ = fs::remove_dir_all(&outside);
}

/// `resolve_finding_path` (`context.md` §8): the working directory first
/// (npm), then each ancestor up to the repository root (cargo), and never a
/// path that names no file.
#[test]
fn a_finding_path_resolves_from_the_working_directory_then_its_ancestors() {
    let repo = IndexedMonorepo::new("task7-finding-paths");
    repo.write("src/shared.ts", "export {};\n");
    repo.write("packages/api/src/shared.ts", "export {};\n");
    let api = repo.root.join("packages/api");

    assert_eq!(
        resolve_finding_path(&repo.root, &api, "src/index.ts"),
        Some("packages/api/src/index.ts".to_string()),
        "npm prints cwd-relative paths"
    );
    assert_eq!(
        resolve_finding_path(
            &repo.root,
            &repo.root.join("crates/engine"),
            "crates/engine/src/lib.rs"
        ),
        Some("crates/engine/src/lib.rs".to_string()),
        "cargo prints workspace-root-relative paths"
    );
    assert_eq!(
        resolve_finding_path(&repo.root, &api, "src/missing.ts"),
        None,
        "no file, no range"
    );
    assert_eq!(
        resolve_finding_path(&repo.root, &api, "src/shared.ts"),
        Some("packages/api/src/shared.ts".to_string()),
        "the working directory wins over an ancestor"
    );
    assert_eq!(
        resolve_finding_path(&repo.root, &api, "/etc/passwd"),
        None,
        "an absolute path is never a repository range"
    );
}

/// A shell script that prints a rustc-shaped error and fails. It is run by
/// the system `sh` (`sh ./cargo check`), never launched as a fresh
/// executable: macOS XProtect can hold a newly written executable for
/// minutes (OBSERVATIONS.md row 25), which would make this test flaky.
const FAKE_CARGO: &str = "cat >&2 <<'EOF'\nerror[E0308]: mismatched types\n --> src/x.ts:1:1\n  |\n\nerror: could not compile `demo` (lib) due to 1 previous error\nEOF\nexit 1\n";

/// A finding from a command run in `packages/api` keeps its range, rewritten
/// to the repository-relative `packages/api/src/x.ts` and hashed from there.
#[test]
fn a_finding_from_a_sub_root_command_keeps_its_range_in_that_root() {
    let repo = IndexedMonorepo::new("task7-finding-range");
    repo.write("packages/api/src/x.ts", "export {};\n");
    repo.write("packages/api/cargo", FAKE_CARGO);
    let config = Config {
        shell: "/bin/sh".to_string(),
        ..repo.config()
    };
    let engine = WorkspaceEngine::new(config);

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"sh ./cargo check","reason":"Check","working_directory":"packages/api"}"#,
    )]]);
    let stopped = chat_turn(&engine, &repo.root, &mut adapter);
    if let Some(card) = &stopped.command_proposal {
        approve(&engine, &card.id);
    }

    let recorded = engine
        .session_store
        .read_findings(&stopped.session.id, &repo.root)
        .unwrap();
    let kept = recorded
        .iter()
        .find(|finding| finding.code() == Some("E0308"))
        .unwrap_or_else(|| panic!("the located finding was recorded: {recorded:?}"));
    assert_eq!(
        kept.range().map(|range| range.path.as_str()),
        Some("packages/api/src/x.ts")
    );
    assert_eq!(
        kept.file_hash(),
        Some(
            workspace_engine::hash::file_hash(repo.root.join("packages/api/src/x.ts"))
                .unwrap()
                .as_str()
        )
    );
}

/// A root-qualified grant authorises `npm test` in `packages/api` and nowhere
/// else (`context.md` §2).
#[test]
fn a_root_qualified_grant_authorises_the_command_in_its_root_only() {
    let repo = IndexedMonorepo::new("task7-grant");
    let config = Config {
        command_allowlist: vec!["cd packages/api && npm test".to_string()],
        ..repo.config()
    };
    let engine = WorkspaceEngine::new(config);

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"npm test","reason":"Test it","working_directory":"packages/api"}"#,
    )]]);
    let in_root = chat_turn(&engine, &repo.root, &mut adapter);
    assert!(
        in_root.command_proposal.is_none(),
        "the grant auto-runs it in packages/api"
    );
    assert!(audited(&engine, "stored_command_executed") >= 1);

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"npm test","reason":"Test it"}"#,
    )]]);
    let at_root = chat_turn(&engine, &repo.root, &mut adapter);
    assert!(
        at_root.command_proposal.is_some(),
        "the grant does not apply at the repository root"
    );
}

/// The resume path re-classifies a proposal at its own directory, so a
/// command whose risk depends on the boundary is judged where it will run.
#[test]
fn a_resumed_proposal_is_re_classified_at_its_own_directory() {
    let repo = IndexedMonorepo::new("task7-resume");
    let engine = WorkspaceEngine::new(repo.config());

    let mut adapter = scripted(vec![vec![tool_call(
        "run_command",
        r#"{"command":"npm test","reason":"Test it","working_directory":"packages/api"}"#,
    )]]);
    let stopped = chat_turn(&engine, &repo.root, &mut adapter);
    let card = stopped.command_proposal.expect("npm test needs approval");

    // Swap in a command that is low risk inside the repository but would be
    // escalated if the boundary were taken to be `packages/api` itself.
    let mut proposal = engine.command_store.load_proposal(&card.id).unwrap();
    proposal.command = "ls ../web/src/index.ts".to_string();
    proposal.requires_approval = true;
    engine.command_store.save_proposal(&proposal).unwrap();
    // Plan allows only a low-risk read-only command, so the classification
    // decides whether the resumed command runs.
    engine
        .session_store
        .set_session_mode(&stopped.session.id, SessionMode::Plan, "user")
        .unwrap();

    approve(&engine, &card.id);
    assert_eq!(
        audited(&engine, "stored_command_executed"),
        1,
        "re-classified at packages/api, inside the repository"
    );
}
