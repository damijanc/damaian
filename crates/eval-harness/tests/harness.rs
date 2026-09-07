use std::path::PathBuf;

#[test]
fn refuses_a_data_dir_inside_the_real_application_support() {
    let home = std::env::var("HOME").expect("HOME should be set");
    let unsafe_dir = PathBuf::from(&home).join("Library/Application Support/DamaianClient/eval");

    let error = eval_harness::guard::assert_safe_data_dir(&unsafe_dir)
        .expect_err("a data dir inside Application Support must be refused");

    assert!(
        format!("{error:?}").contains("Application Support"),
        "the refusal should name what it refused, got: {error:?}"
    );
}

#[test]
fn accepts_a_temporary_data_dir() {
    let dir = eval_harness::guard::eval_data_dir().expect("temp data dir should be created");
    assert!(dir.is_dir(), "the data dir should exist");
    eval_harness::guard::assert_safe_data_dir(&dir).expect("a temp dir must be accepted");
}

#[test]
fn materializing_a_fixture_produces_a_real_git_repository() {
    let fixture = eval_harness::fixture::materialize("rust-workspace")
        .expect("the rust-workspace fixture should materialize");

    assert_eq!(fixture.version, "1");
    assert!(
        fixture.root.join("src/upload.rs").is_file(),
        "tree should be copied"
    );
    assert!(
        fixture.root.join(".git").is_dir(),
        "fixture should be a git repository"
    );
    assert!(
        !fixture.data_dir.starts_with(&fixture.root),
        "data dir must sit outside the repo"
    );

    // A committed tree, not just an initialized one: Damaian reads git status,
    // and an uncommitted tree would make every scenario see spurious changes.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&fixture.root)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status should run");
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "a freshly materialized fixture should have a clean working tree"
    );
}

#[test]
fn two_materializations_are_independent() {
    let first = eval_harness::fixture::materialize("rust-workspace").expect("first");
    let second = eval_harness::fixture::materialize("rust-workspace").expect("second");
    assert_ne!(first.root, second.root, "each run needs its own copy");

    std::fs::write(first.root.join("src/upload.rs"), "// clobbered").expect("write");
    let untouched = std::fs::read_to_string(second.root.join("src/upload.rs")).expect("read");
    assert!(
        untouched.contains("pub fn upload"),
        "runs must not share state"
    );
}
