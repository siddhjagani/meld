//! Run `meld watch` in the background as a macOS LaunchAgent: started at
//! login, restarted if it exits, no terminal needed.

use crate::config::home_dir;
use crate::errors::Result;
use crate::logging;
use anyhow::Context;
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.meld.watch";

fn plist_path() -> PathBuf {
    home_dir().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn log_path() -> PathBuf {
    home_dir().join("Library/Logs/meld.log")
}

#[cfg(unix)]
fn uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

fn domain() -> String {
    format!("gui/{}", uid())
}

fn ensure_macos() -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("the background service is only available on macOS — run `meld watch` instead");
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Write the LaunchAgent plist and (re)load it.
pub fn install() -> Result<()> {
    ensure_macos()?;
    // Not canonicalized on purpose: a Homebrew symlink (/opt/homebrew/bin/meld)
    // survives `brew upgrade`, the versioned Cellar path it points to does not.
    let exe = std::env::current_exe().context("locating the meld executable")?;
    let exe = if exe.is_absolute() { exe } else { std::env::current_dir()?.join(exe) };
    let log = log_path();
    let plist = render_plist(&exe, &log);

    let path = plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;

    // Reload so a reinstall picks up a new binary path.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain()])
        .arg(&path)
        .output()
        .context("running launchctl")?;
    if !out.status.success() {
        anyhow::bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    logging::info("Background sync is on. It starts at login and keeps every account in sync.");
    logging::info(&format!("Log: {}", log.display()));
    Ok(())
}

/// The LaunchAgent definition for `meld watch`.
pub fn render_plist(exe: &std::path::Path, log: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>watch</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>30</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        exe = xml_escape(&exe.display().to_string()),
        log = xml_escape(&log.display().to_string()),
    )
}

/// Stop the agent and remove its plist.
pub fn uninstall() -> Result<()> {
    ensure_macos()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .output();
    let path = plist_path();
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    logging::info("Background sync is off.");
    Ok(())
}

/// Whether the agent is installed and running.
pub fn status() -> Result<()> {
    ensure_macos()?;
    let installed = plist_path().exists();
    let out = Command::new("launchctl")
        .args(["print", &format!("{}/{LABEL}", domain())])
        .output()
        .context("running launchctl")?;
    let loaded = out.status.success();
    let pid = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("pid = ").map(str::to_string));

    match (installed, loaded, pid) {
        (true, true, Some(pid)) => println!("Background sync is running (pid {pid})."),
        (true, true, None) => println!("Background sync is installed but not running right now (see the log)."),
        (true, false, _) => println!("Background sync is installed but not loaded. Run `meld service install` again."),
        (false, _, _) => println!("Background sync is off. Turn it on with `meld service install`."),
    }
    println!("Log: {}", log_path().display());
    Ok(())
}
