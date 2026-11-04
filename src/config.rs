use crate::errors::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Meld configuration. Lives at ~/.meld/config.toml; every field has a
/// sensible default so the tool works with no config file at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Parent directory that contains one folder per account.
    pub sessions_root: PathBuf,
    /// Where meld keeps its manifest, backups and lock file.
    pub state_dir: PathBuf,
    /// File-watcher debounce in milliseconds.
    pub debounce_ms: u64,
    /// Keep at most this many snapshots (oldest pruned first).
    pub max_snapshots: usize,
    /// Full snapshots copy every account tree (tens of MB each), so they get
    /// their own, smaller cap on top of `max_snapshots`.
    pub max_full_snapshots: usize,
    /// File name patterns to ignore (exact basename match).
    pub ignore: Vec<String>,
    /// What happens to a conversation you delete in one account or
    /// organization. Default: it comes back from the others.
    pub deleted_chats: DeletedChats,
    /// Older setting: `true` means `deleted_chats = "delete_everywhere"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub propagate_deletes: Option<bool>,
    /// On by default: when accounts hold different versions of a conversation,
    /// the copy that was edited replaces the others (the replaced copies are
    /// backed up). Set to false to keep both versions as a conflict.
    pub propagate_updates: bool,
}

/// What a sync does with a conversation that was deleted somewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletedChats {
    /// Copied back from the accounts that still have it (nothing is deleted).
    #[default]
    ComeBack,
    /// Stays deleted in the organization you deleted it in; every other
    /// account and organization keeps it.
    StayDeletedHere,
    /// Deleted in every account and organization (copies are backed up).
    DeleteEverywhere,
}

/// The parts of the config that decide what a sync does.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub deletes: DeletedChats,
    pub propagate_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sessions_root: default_sessions_root(),
            state_dir: home_dir().join(".meld"),
            debounce_ms: 1500,
            max_snapshots: 200,
            max_full_snapshots: 10,
            ignore: vec![".DS_Store".into(), "desktop.ini".into(), "Thumbs.db".into()],
            deleted_chats: DeletedChats::ComeBack,
            propagate_deletes: None,
            propagate_updates: true,
        }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// The per-user state base on Windows: `%LOCALAPPDATA%` (the `Local` folder).
/// On macOS this is `~/Library/Application Support`, on Linux `~/.config`.
fn local_app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join("AppData").join("Local"))
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().join("Library/Application Support")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".config")
    }
}

/// The roaming per-user state base on Windows: `%APPDATA%`.
/// Only defined on Windows; other platforms fall back to the local base.
#[cfg(target_os = "windows")]
fn roaming_app_data_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join("AppData").join("Roaming"))
}

/// First-party Claude install: `<app-data>/Claude/claude-code-sessions`.
fn first_party_root(app_data: &Path) -> PathBuf {
    app_data.join("Claude").join("claude-code-sessions")
}

/// Third-party inference profile store: `<app-data>/Claude-3p/claude-code-sessions`.
/// Exists on Windows (`%LOCALAPPDATA%\Claude-3p\` and `%APPDATA%\Claude-3p\`),
/// macOS (`~/Library/Application Support/Claude-3p/`) and Linux alike.
fn third_party_root(app_data: &Path) -> PathBuf {
    app_data.join("Claude-3p").join("claude-code-sessions")
}

/// True if the root holds at least one session file (`local_<chat>.json`).
/// A directory that exists but only holds a stub (e.g. `profile-origin.json`)
/// is not populated and must not shadow a populated store.
fn has_sessions(root: &Path) -> bool {
    let Ok(accounts) = std::fs::read_dir(root) else {
        return false;
    };
    for acct in accounts.flatten() {
        let Ok(orgs) = std::fs::read_dir(acct.path()) else {
            continue;
        };
        for org in orgs.flatten() {
            let Ok(files) = std::fs::read_dir(org.path()) else {
                continue;
            };
            if files.flatten().any(|f| {
                f.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("local_") && n.ends_with(".json"))
            }) {
                return true;
            }
        }
    }
    false
}

/// All candidate session-store locations, most preferred first.
/// On Windows both `%LOCALAPPDATA%` and `%APPDATA%` are checked, first-party
/// then third-party for each — so all four combos are covered. meld scans
/// every candidate it finds — a user can legitimately have history in both
/// the first-party and third-party stores.
fn candidates_sessions_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let local = local_app_data_dir();
        let roaming = roaming_app_data_dir();
        vec![
            first_party_root(&local),
            third_party_root(&local),
            first_party_root(&roaming),
            third_party_root(&roaming),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        let base = local_app_data_dir();
        vec![first_party_root(&base), third_party_root(&base)]
    }
}

/// Platform default for the Claude Code desktop session store. Prefers the
/// first candidate that actually holds session files, falls back to the first
/// one that exists, then to the first-party local path as a sensible default.
fn default_sessions_root() -> PathBuf {
    let candidates = candidates_sessions_roots();
    for c in &candidates {
        if has_sessions(c) {
            return c.clone();
        }
    }
    candidates
        .iter()
        .find(|c| c.exists())
        .cloned()
        .unwrap_or_else(|| first_party_root(&local_app_data_dir()))
}

pub fn config_path() -> PathBuf {
    home_dir().join(".meld/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meld-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn default_sessions_root_prefers_populated_store() {
        let base = tmp_dir("prefers_populated");

        let first_party = base.join("first_party");
        fs::create_dir_all(&first_party).unwrap();
        // first-party is empty (stub only) — not populated
        fs::write(first_party.join("profile-origin.json"), "{}").unwrap();
        assert!(!has_sessions(&first_party));

        let org_root = base.join("third_party").join("account").join("org");
        fs::create_dir_all(&org_root).unwrap();
        fs::write(org_root.join("local_x.json"), r#"{}"#).unwrap();
        assert!(has_sessions(&org_root.parent().unwrap().parent().unwrap()));
    }

    #[test]
    fn has_sessions_ignores_non_session_files() {
        let base = tmp_dir("ignores_non_session");
        let org = base.join("acct").join("org");
        fs::create_dir_all(&org).unwrap();
        fs::write(org.join("scheduled-tasks.json"), "[]").unwrap();
        fs::write(org.join("deleted_x"), "marker").unwrap();
        assert!(!has_sessions(&base));

        fs::write(org.join("local_x.json"), r#"{}"#).unwrap();
        assert!(has_sessions(&base));
    }

    #[test]
    fn has_sessions_returns_false_for_missing_dir() {
        assert!(!has_sessions(&tmp_dir("missing_dir").join("nope")));
    }
}

impl Config {
    /// Load from ~/.meld/config.toml, or defaults if absent.
    pub fn load() -> Result<Config> {
        let path = config_path();
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading config {}", path.display()))?;
            let cfg: Config = toml::from_str(&raw)
                .with_context(|| format!("parsing config {}", path.display()))?;
            Ok(cfg)
        } else {
            Ok(Config::default())
        }
    }

    /// Write the default config to ~/.meld/config.toml (no overwrite).
    pub fn init() -> Result<PathBuf> {
        let path = config_path();
        if path.exists() {
            anyhow::bail!("config already exists at {}", path.display());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cfg = Config::default();
        std::fs::write(&path, toml::to_string_pretty(&cfg)?)?;
        Ok(path)
    }

    pub fn policy(&self) -> Policy {
        Policy {
            deletes: if self.propagate_deletes == Some(true) {
                DeletedChats::DeleteEverywhere
            } else {
                self.deleted_chats
            },
            propagate_updates: self.propagate_updates,
        }
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.state_dir.join("manifest.json")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("meld.lock")
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| self.ignore.iter().any(|i| i == n))
            .unwrap_or(false)
    }
}
