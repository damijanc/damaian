//! The CLI refuses a data directory this build cannot read, per
//! `docs/specs/15_install_and_update_verification.md` §5.2.
//!
//! The desktop app is not the only way into the data directory. A CLI that
//! carried on where the app refuses would be the hole the refusal exists to
//! close — an older build writing over data a newer one reorganised.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_data_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("damaian-cli-schema-{name}-{stamp}"));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

#[test]
fn cli_refuses_a_data_directory_written_by_a_newer_schema() {
    let data_dir = temp_data_dir("newer");
    fs::write(data_dir.join("schema.conf"), "schema_version=999\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_damaian"))
        .arg("config-show")
        .env("DAMAIAN_DATA_DIR", &data_dir)
        .output()
        .expect("cli should run");

    assert!(!output.status.success(), "the cli should refuse to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&data_dir.display().to_string()) && stderr.contains("999"),
        "refusal should name the directory and the version found: {stderr}"
    );
}

#[test]
fn cli_marks_a_fresh_data_directory_with_the_current_schema() {
    let data_dir = temp_data_dir("fresh");

    let output = Command::new(env!("CARGO_BIN_EXE_damaian"))
        .arg("config-show")
        .env("DAMAIAN_DATA_DIR", &data_dir)
        .output()
        .expect("cli should run");

    assert!(
        output.status.success(),
        "a fresh data directory should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(data_dir.join("schema.conf")).expect("marker should be written"),
        "schema_version=1\n"
    );
}
