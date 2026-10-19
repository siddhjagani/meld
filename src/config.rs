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

/// Platform default for the Claude Code desktop session store.
fn default_sessions_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join("AppData").join("Roaming"))
            .join("Claude")
            .join("claude-code-sessions")
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().join("Library/Application Support/Claude/claude-code-sessions")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".config/Claude/claude-code-sessions")
    }
}

pub fn config_path() -> PathBuf {
    home_dir().join(".meld/config.toml")
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
