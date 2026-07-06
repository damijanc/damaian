#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use desktop_shell::{ShellOptions, run_server_with_ready};
use tauri::{Manager, Url};

const SHELL_HOST: &str = "127.0.0.1";
const PREFERRED_SHELL_PORT: u16 = 4765;

fn main() {
    let shell_options = ShellOptions::new(shell_port(), repo_from_args_or_env());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
