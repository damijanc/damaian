#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use desktop_shell::{ShellOptions, run_server_with_ready};
use serde::Serialize;
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Manager, Runtime, Url};

const SHELL_HOST: &str = "127.0.0.1";
const PREFERRED_SHELL_PORT: u16 = 4765;
const SETTINGS_MENU_ID: &str = "damaian-settings";
const CHECK_FOR_UPDATES_MENU_ID: &str = "damaian-check-for-updates";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopBootstrap {
    api_token: String,
    default_repo: Option<String>,
}

fn main() {
    let shell_port = shell_port().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    let shell_options = ShellOptions::new(shell_port, repo_from_args_or_env());
    let bootstrap = DesktopBootstrap {
        api_token: shell_options.api_token.clone(),
        default_repo: shell_options.default_repo.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(bootstrap)
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            if event.id() == SETTINGS_MENU_ID {
                open_settings(app);
            }
            if event.id() == CHECK_FOR_UPDATES_MENU_ID {
                check_for_updates(app);
            }
        })
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
        .invoke_handler(tauri::generate_handler![damaian_desktop_bootstrap])
        .run(tauri::generate_context!())
        .expect("error while running Damaian desktop app");
}

#[tauri::command]
fn damaian_desktop_bootstrap(bootstrap: tauri::State<'_, DesktopBootstrap>) -> DesktopBootstrap {
    bootstrap.inner().clone()
}

fn build_app_menu<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    #[cfg(target_os = "macos")]
    {
        build_macos_menu(app)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Menu::default(app)
    }
}

#[cfg(target_os = "macos")]
fn build_macos_menu<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    let app_name = app.package_info().name.clone();
    let about_metadata = AboutMetadata {
        name: Some(app_name.clone()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };

    let about = PredefinedMenuItem::about(app, None, Some(about_metadata))?;
    let check_for_updates = MenuItem::with_id(
        app,
        CHECK_FOR_UPDATES_MENU_ID,
        "Check for Updates...",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        SETTINGS_MENU_ID,
        "Settings...",
        true,
        Some("CmdOrCtrl+,"),
    )?;
    let services = PredefinedMenuItem::services(app, None)?;
    let hide = PredefinedMenuItem::hide(app, None)?;
    let hide_others = PredefinedMenuItem::hide_others(app, None)?;
    let show_all = PredefinedMenuItem::show_all(app, None)?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    let app_menu = Submenu::with_items(
        app,
        app_name,
        true,
        &[
            &about,
            &PredefinedMenuItem::separator(app)?,
            &check_for_updates,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &services,
            &PredefinedMenuItem::separator(app)?,
            &hide,
            &hide_others,
            &show_all,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let close_window = PredefinedMenuItem::close_window(app, None)?;
    let file_menu = Submenu::with_items(app, "File", true, &[&close_window])?;

    let undo = PredefinedMenuItem::undo(app, None)?;
    let redo = PredefinedMenuItem::redo(app, None)?;
    let cut = PredefinedMenuItem::cut(app, None)?;
    let copy = PredefinedMenuItem::copy(app, None)?;
    let paste = PredefinedMenuItem::paste(app, None)?;
    let select_all = PredefinedMenuItem::select_all(app, None)?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &undo,
            &redo,
            &PredefinedMenuItem::separator(app)?,
            &cut,
            &copy,
            &paste,
            &PredefinedMenuItem::separator(app)?,
            &select_all,
        ],
    )?;

    let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
    let view_menu = Submenu::with_items(app, "View", true, &[&fullscreen])?;

    let minimize = PredefinedMenuItem::minimize(app, None)?;
    let maximize = PredefinedMenuItem::maximize(app, None)?;
    let close_window = PredefinedMenuItem::close_window(app, None)?;
    let bring_all_to_front = PredefinedMenuItem::bring_all_to_front(app, None)?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &minimize,
            &maximize,
            &PredefinedMenuItem::separator(app)?,
            &close_window,
            &PredefinedMenuItem::separator(app)?,
            &bring_all_to_front,
        ],
    )?;

    Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )
}

fn open_settings<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.dispatchEvent(new Event('damaian-open-settings'))");
        let _ = window.set_focus();
    }
}

fn check_for_updates<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.dispatchEvent(new Event('damaian-check-for-updates'))");
        let _ = window.set_focus();
    }
}

fn shell_port() -> Result<u16, String> {
    shell_port_for_status(TcpStream::connect((SHELL_HOST, PREFERRED_SHELL_PORT)).is_ok())
}

fn shell_port_for_status(preferred_port_in_use: bool) -> Result<u16, String> {
    if preferred_port_in_use {
        Err(format!(
            "Damaian desktop shell refuses to start because {SHELL_HOST}:{PREFERRED_SHELL_PORT} is already in use. The Tauri capability is scoped to that exact origin; stop the conflicting process and restart Damaian."
        ))
    } else {
        Ok(PREFERRED_SHELL_PORT)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_port_uses_preferred_port_when_available() {
        assert_eq!(shell_port_for_status(false), Ok(PREFERRED_SHELL_PORT));
    }

    #[test]
    fn shell_port_refuses_fallback_when_preferred_port_is_busy() {
        let error = shell_port_for_status(true).expect_err("busy port should refuse startup");

        assert!(error.contains("already in use"));
        assert!(error.contains("4765"));
    }
}
