//! Repository map and monorepo boundaries, per
//! `docs/specs/24_repository_map_and_monorepo_boundaries/proposal.md`.
//!
//! Task 1 pins root detection. It is a pure function of the index's file
//! and skip lists (`context.md` §3), so only the test that ties the manifest
//! list to `detect_project_commands` touches the filesystem.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::indexer::SkippedFile;
use workspace_engine::{
    CommandPolicy, Config, DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH,
    PROJECT_MANIFESTS, RootDetection, RootEvidence, detect_roots,
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
