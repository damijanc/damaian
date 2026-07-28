//! Real PTY-backed terminal sessions.
//!
//! Each session owns a login shell running on its own pseudo-terminal, so the
//! shell's own line editor drives history (up/down), reverse search (Ctrl+R)
//! and tab completion exactly as it would in a standalone terminal. Output is
//! streamed to the frontend over the existing SSE transport and keystrokes are
//! written back to the pty master.

use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::thread;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Largest keystroke/output payload we will move in a single message. Guards
/// against a pathological client flooding the pty.
const MAX_INPUT_BYTES: usize = 1 << 20;

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Output channel, taken by the streaming endpoint on first connect.
    receiver: Option<Receiver<Vec<u8>>>,
}

fn sessions() -> &'static Mutex<HashMap<String, PtySession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, PtySession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_session_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("secure random terminal id generation failed");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, PtySession>> {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Spawn a login shell on a fresh pty in `cwd` and return its session id.
pub fn open(cwd: &Path, cols: u16, rows: u16) -> Result<String, String> {
    let size = PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|error| format!("failed to open pty: {error}"))?;

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l");
    cmd.cwd(cwd);
    // Inherit the user's environment so PATH and friends are present, then
    // advertise a capable terminal so the shell emits colour/cursor sequences.
    for (key, value) in env::vars() {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|error| format!("failed to start shell: {error}"))?;
    // Dropping the slave ensures the master sees EOF once the shell exits.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to read from pty: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to write to pty: {error}"))?;

    let (sender, receiver) = channel::<Vec<u8>>();
    spawn_reader(reader, sender);

    let id = new_session_id();
    lock().insert(
        id.clone(),
        PtySession {
            master: pair.master,
            writer,
            child,
            receiver: Some(receiver),
        },
    );
    Ok(id)
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, sender: Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // Dropping `sender` closes the channel and unblocks the stream loop.
    });
}

/// Hand the output channel to the streaming endpoint. Only the first caller
/// for a given session succeeds.
pub fn take_output(id: &str) -> Result<Receiver<Vec<u8>>, String> {
    let mut guard = lock();
    let session = guard
        .get_mut(id)
        .ok_or_else(|| "terminal session not found".to_string())?;
    session
        .receiver
        .take()
        .ok_or_else(|| "terminal session is already being streamed".to_string())
}

/// Forward keystrokes to the shell.
pub fn write_input(id: &str, data: &[u8]) -> Result<(), String> {
    if data.len() > MAX_INPUT_BYTES {
        return Err("terminal input too large".to_string());
    }
    let mut guard = lock();
    let session = guard
        .get_mut(id)
        .ok_or_else(|| "terminal session not found".to_string())?;
    session
        .writer
        .write_all(data)
        .and_then(|_| session.writer.flush())
        .map_err(|error| format!("failed to write to terminal: {error}"))
}

/// Resize the pty when the xterm viewport changes.
pub fn resize(id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let guard = lock();
    let session = guard
        .get(id)
        .ok_or_else(|| "terminal session not found".to_string())?;
    session
        .master
        .resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("failed to resize terminal: {error}"))
}

/// Kill the shell, drop the session and return its exit code (best effort).
pub fn close(id: &str) -> Option<i32> {
    let mut session = lock().remove(id)?;
    let _ = session.child.kill();
    let code = session
        .child
        .wait()
        .ok()
        .map(|status| status.exit_code() as i32);
    Some(code.unwrap_or(-1))
}
