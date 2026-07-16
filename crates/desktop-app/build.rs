fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_UPDATER_PUBKEY");
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["damaian_desktop_bootstrap"]),
        ),
    )
    .expect("failed to run tauri-build");
}
