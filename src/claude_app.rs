//! Quit and relaunch the Claude desktop app around a sync (macOS).
//!
//! Claude reads the conversation list of the signed-in account into memory
//! and saves from that copy, so files meld writes while it runs are not
//! shown (and can be overwritten) until the list is reloaded. Quitting first
//! makes Claude save everything to disk; relaunching makes it read the
//! synced files.

use crate::errors::Result;
use crate::logging;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long to wait for Claude to save and exit before giving up.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The app to control. `MELD_CLAUDE_APP` overrides it (used by tests).
fn app_name() -> String {
    std::env::var("MELD_CLAUDE_APP").unwrap_or_else(|_| "Claude".into())
}

pub fn is_running() -> bool {
    Command::new("pgrep")
        .args(["-x", &app_name()])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Ask Claude to quit (the normal way, so it saves its state) and wait until
/// it is gone. Never force-kills: if it doesn't exit, this fails and nothing
/// is synced. Returns whether it was running.
pub fn quit_and_wait() -> Result<bool> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("--restart-claude is only available on macOS");
    }
    if !is_running() {
        return Ok(false);
    }
    let name = app_name();
    logging::info(&format!("Quitting {name} so it saves your conversations first..."));
    let out = Command::new("osascript")
        .args(["-e", &format!("quit app \"{name}\"")])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "could not ask {name} to quit: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let start = Instant::now();
    while is_running() {
        if start.elapsed() > QUIT_TIMEOUT {
            anyhow::bail!("{name} did not quit within {}s (is a dialog open?); nothing was synced", QUIT_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(true)
}

pub fn launch() -> Result<()> {
    let name = app_name();
    let out = Command::new("open").args(["-a", &name]).output()?;
    if !out.status.success() {
        anyhow::bail!("could not reopen {name}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    logging::info(&format!("Reopened {name}: it now shows the synced conversations."));
    Ok(())
}
