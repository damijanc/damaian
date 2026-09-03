fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_UPDATER_PUBKEY");
    // The release channel is baked in with option_env!, which Cargo does not
    // track. Without this, a cached target/ directory can carry a preview
    // binary into a stable build at the same version.
    println!("cargo:rerun-if-env-changed=DAMAIAN_RELEASE_CHANNEL");
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "damaian_desktop_bootstrap",
            "terminal_open",
            "terminal_write",
            "terminal_resize",
            "terminal_close",
        ]),
    ))
    .expect("failed to run tauri-build");
}
