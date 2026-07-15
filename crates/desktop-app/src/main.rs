#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use desktop_shell::{ShellOptions, run_server_with_ready};
use serde::Serialize;
use tauri::{Manager, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

const SHELL_HOST: &str = "127.0.0.1";
const PREFERRED_SHELL_PORT: u16 = 4765;
const UPDATER_ENDPOINT: &str =
    "https://github.com/damijanc/damaian/releases/latest/download/latest.json";

struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckResult {
    configured: bool,
    available: bool,
    version: Option<String>,
    current_version: Option<String>,
    message: Option<String>,
}

fn main() {
    let shell_options = ShellOptions::new(shell_port(), repo_from_args_or_env());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(PendingUpdate(Mutex::new(None)))
        .setup(move |app| {
            let options = shell_options.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            thread::spawn(move || {
                let startup_tx = ready_tx.clone();
                if let Err(error) = run_server_with_ready(options, move |port| {
                    let _ = startup_tx.send(Ok(port));
                }) {
                    let _ = ready_tx.send(Err(error.clone()));
                    eprintln!("Damaian shell server stopped: {error}");
                }
            });

            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(port)) => {
                    let shell_url = format!("http://{SHELL_HOST}:{port}/");
                    if let Some(window) = app.get_webview_window("main") {
                        match Url::parse(&shell_url) {
                            Ok(url) => {
                                if let Err(error) = window.navigate(url) {
                                    eprintln!("Damaian shell navigation failed: {error}");
                                }
                            }
                            Err(error) => eprintln!("Damaian shell URL is invalid: {error}"),
                        }
                    } else {
                        eprintln!("Damaian main window was not available for shell navigation");
                    }
                }
                Ok(Err(error)) => eprintln!("Damaian shell did not start: {error}"),
                Err(error) => eprintln!("Damaian shell did not report readiness: {error}"),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            damaian_check_for_update,
            damaian_install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running Damaian desktop app");
}

fn shell_port() -> u16 {
    if TcpStream::connect((SHELL_HOST, PREFERRED_SHELL_PORT)).is_ok() {
        0
    } else {
        PREFERRED_SHELL_PORT
    }
}

fn repo_from_args_or_env() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--repo" {
            return args.next();
        }
    }
    env::var("DAMAIAN_REPO")
        .ok()
        .filter(|value| !value.is_empty())
}

#[tauri::command]
async fn damaian_check_for_update(
    app: tauri::AppHandle,
    pending_update: tauri::State<'_, PendingUpdate>,
) -> Result<UpdateCheckResult, String> {
    let Some(pubkey) = updater_pubkey() else {
        return Ok(UpdateCheckResult {
            configured: false,
            available: false,
            version: None,
            current_version: None,
            message: Some("Updater public key is not configured in this build".to_string()),
        });
    };

    let update = app
        .updater_builder()
        .pubkey(pubkey)
        .endpoints(vec![
            UPDATER_ENDPOINT
                .parse::<Url>()
                .map_err(|error| error.to_string())?,
        ])
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;

    let result = update
        .as_ref()
        .map(|update| UpdateCheckResult {
            configured: true,
            available: true,
            version: Some(update.version.clone()),
            current_version: Some(update.current_version.clone()),
            message: None,
        })
        .unwrap_or(UpdateCheckResult {
            configured: true,
            available: false,
            version: None,
            current_version: None,
            message: Some("Damaian is up to date".to_string()),
        });

    *pending_update
        .0
        .lock()
        .map_err(|_| "Pending update state is unavailable".to_string())? = update;
    Ok(result)
}

#[tauri::command]
async fn damaian_install_update(
    app: tauri::AppHandle,
    pending_update: tauri::State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = {
        let mut guard = pending_update
            .0
            .lock()
            .map_err(|_| "Pending update state is unavailable".to_string())?;
        guard.take()
    }
    .ok_or_else(|| "No pending update. Check for updates first.".to_string())?;

    update
        .download_and_install(|_chunk_length, _content_length| {}, || {})
        .await
        .map_err(|error| error.to_string())?;

    app.restart();
}

fn updater_pubkey() -> Option<&'static str> {
    option_env!("TAURI_UPDATER_PUBKEY")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
