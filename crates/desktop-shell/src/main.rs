use workspace_engine::{Config, ProcessRegistry};

fn main() {
    if let Err(error) = install_shutdown_handler() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    if let Err(error) = desktop_shell::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Kills this instance's registered children when the shell is signalled, per
/// `docs/specs/46_process_registry_and_orphan_sweep/proposal.md` §5.6.
///
/// Installed from `main` and never from the library: the Tauri host and the
/// test harness both link `desktop_shell` and must keep their own signal
/// dispositions. The launch sweep is deliberately not here — it belongs to
/// `run_server_with_ready`, so the Tauri host gets it too.
fn install_shutdown_handler() -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let (registry, audit) =
        ProcessRegistry::open_with_audit(&config).map_err(|error| error.to_string())?;
    registry
        .install_shutdown_handler(audit)
        .map_err(|error| error.to_string())
}
