//! Run `meld watch` in the background: started at login, restarted if it
//! exits, no terminal needed. A LaunchAgent on macOS; on Windows a per-user
//! login entry (see the `windows` module).

use crate::config::{home_dir, Config};
use crate::errors::Result;
use crate::logging;
use anyhow::Context;
use std::path::{Path, PathBuf};
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
    unsafe extern "C" {
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
        anyhow::bail!("the background service is only available on macOS and Windows — run `meld watch` instead");
    }
    Ok(())
}

/// The executable to start at login. Not canonicalized on purpose: a Homebrew
/// symlink (/opt/homebrew/bin/meld) survives `brew upgrade`, the versioned
/// Cellar path it points to does not.
fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the meld executable")?;
    Ok(if exe.is_absolute() { exe } else { std::env::current_dir()?.join(exe) })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Turn background sync on: now and at every login.
pub fn install(cfg: &Config) -> Result<()> {
    if cfg!(windows) {
        return windows::install(cfg);
    }
    ensure_macos()?;
    let exe = exe_path()?;
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
pub fn render_plist(exe: &Path, log: &Path) -> String {
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

/// Stop background sync and remove it from login.
pub fn uninstall(cfg: &Config) -> Result<()> {
    if cfg!(windows) {
        return windows::uninstall(cfg);
    }
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

/// Whether background sync is installed and running.
pub fn status(cfg: &Config) -> Result<()> {
    if cfg!(windows) {
        return windows::status(cfg);
    }
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

/// The process the Windows login entry starts (`meld service run`).
pub fn run(cfg: &Config) -> Result<()> {
    windows::run(cfg)
}

pub use windows::render_login_command;

/// Windows: `HKCU\...\Run` starts `conhost --headless meld service run` at
/// sign-in. Headless conhost gives meld a console without a window (Task
/// Scheduler or the Run key would otherwise show a console window the whole
/// time). `service run` keeps `meld watch` alive with its output in
/// `~/.meld/meld.log`, like launchd's KeepAlive + log redirection. No admin
/// rights needed. Not cfg-gated so it stays compiled and tested on macOS.
mod windows {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::process::Stdio;
    use std::time::Duration;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "meld";
    /// Wait this long before restarting `meld watch` after it exits.
    const RESTART_SECS: u64 = 30;

    fn log_path(cfg: &Config) -> PathBuf {
        cfg.state_dir.join("meld.log")
    }

    fn pid_path(cfg: &Config) -> PathBuf {
        cfg.state_dir.join("service.pid")
    }

    fn conhost() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join("System32")
            .join("conhost.exe")
    }

    /// The command line stored in the Run key.
    pub fn render_login_command(conhost: &Path, exe: &Path) -> String {
        format!("\"{}\" --headless \"{}\" service run", conhost.display(), exe.display())
    }

    pub fn install(cfg: &Config) -> Result<()> {
        let exe = exe_path()?;
        std::fs::create_dir_all(&cfg.state_dir)?;
        let line = render_login_command(&conhost(), &exe);
        let out = Command::new("reg")
            .args(["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &line, "/f"])
            .output()
            .context("running reg")?;
        if !out.status.success() {
            anyhow::bail!(
                "could not add meld to your sign-in programs: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        // Restart so a reinstall picks up a new binary path.
        stop(cfg);
        let mut start = Command::new(conhost());
        start.arg("--headless").arg(&exe).args(["service", "run"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x8;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
            // Outlive this terminal and ignore its Ctrl-C.
            start.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        start
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting background sync")?;
        logging::info("Background sync is on. It starts when you sign in and keeps every account in sync.");
        logging::info(&format!("Log: {}", log_path(cfg).display()));
        Ok(())
    }

    pub fn uninstall(cfg: &Config) -> Result<()> {
        let _ = Command::new("reg").args(["delete", RUN_KEY, "/v", VALUE, "/f"]).output();
        stop(cfg);
        logging::info("Background sync is off.");
        Ok(())
    }

    pub fn status(cfg: &Config) -> Result<()> {
        let installed = Command::new("reg")
            .args(["query", RUN_KEY, "/v", VALUE])
            .output()
            .is_ok_and(|o| o.status.success());
        match (installed, running_pid(cfg)) {
            (true, Some(pid)) => println!("Background sync is running (pid {pid})."),
            (true, None) => println!("Background sync is on but not running right now (see the log). It starts at your next sign-in, or run `meld service install` to start it now."),
            (false, Some(pid)) => println!("Background sync is running (pid {pid}) but won't start at sign-in. Run `meld service install` again."),
            (false, None) => println!("Background sync is off. Turn it on with `meld service install`."),
        }
        println!("Log: {}", log_path(cfg).display());
        Ok(())
    }

    /// Pid of the running `meld service run`, if its pid file names a live
    /// meld process. A stale pid may have been reused by another program, so
    /// the image name must match before anything acts on it.
    fn running_pid(cfg: &Config) -> Option<u32> {
        let pid: u32 = std::fs::read_to_string(pid_path(cfg)).ok()?.trim().parse().ok()?;
        let exe = std::env::current_exe().ok()?;
        let name = exe.file_name()?.to_str()?.to_ascii_lowercase();
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .to_ascii_lowercase()
            .contains(&format!("\"{name}\""))
            .then_some(pid)
    }

    /// Stop `meld service run` and the `meld watch` it started (/T). Writes
    /// are temp-file + rename, so stopping mid-sync can't corrupt a session.
    fn stop(cfg: &Config) {
        if let Some(pid) = running_pid(cfg) {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output();
        }
        let _ = std::fs::remove_file(pid_path(cfg));
    }

    /// Keep `meld watch` running with its output appended to the log.
    pub fn run(cfg: &Config) -> Result<()> {
        if running_pid(cfg).is_some_and(|p| p != std::process::id()) {
            return Ok(()); // already running
        }
        std::fs::create_dir_all(&cfg.state_dir)?;
        std::fs::write(pid_path(cfg), std::process::id().to_string())?;
        let exe = std::env::current_exe().context("locating the meld executable")?;
        let log = log_path(cfg);
        loop {
            let out = OpenOptions::new().create(true).append(true).open(&log)?;
            let status = Command::new(&exe).arg("watch").stdout(out.try_clone()?).stderr(out).status();
            if let Ok(mut f) = OpenOptions::new().append(true).open(&log) {
                let why = match status {
                    Ok(s) => s.to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = writeln!(
                    f,
                    "{} meld watch stopped ({why}); restarting in {RESTART_SECS}s",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                );
            }
            std::thread::sleep(Duration::from_secs(RESTART_SECS));
        }
    }
}
