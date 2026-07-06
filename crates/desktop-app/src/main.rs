#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use desktop_shell::{ShellOptions, run_server};

const SHELL_PORT: u16 = 4765;

fn main() {
    let shell_options = ShellOptions::new(SHELL_PORT, repo_from_args_or_env());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |_app| {
            let options = shell_options.clone();
            thread::spawn(move || {
                if let Err(error) = run_server(options) {
                    eprintln!("Damaian shell server stopped: {error}");
                }
            });

            if !wait_for_shell(SHELL_PORT) {
                eprintln!("Damaian shell did not respond before the window opened");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Damaian desktop app");
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

fn wait_for_shell(port: u16) -> bool {
    for _ in 0..40 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}
