use crate::errors::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One indexed session file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// Absolute path of the account root this file lives under.
    pub account_root: PathBuf,
    /// Absolute path of the file.
    pub file_path: PathBuf,
    /// Path relative to the account root (`<org>/local_<chat>.json`).
    pub relative_path: PathBuf,
    pub sha256: String,
    /// Hash of the JSON content with volatile bookkeeping fields
    /// (lastFocusedAt, lastActivityAt, ...) removed. Two files with equal
    /// content_hash are the same conversation, even if activity timestamps
    /// drifted between accounts. Falls back to sha256 for unparseable files.
    #[serde(default)]
    pub content_hash: String,
    pub size: u64,
    /// mtime as seconds since the unix epoch.
    pub mtime: i64,
    /// Session id inferred from JSON content, falling back to filename.
    pub session_id: Option<String>,
    /// Conversation title from the session JSON, when present.
    #[serde(default)]
    pub title: Option<String>,
    /// Working directory of the session, when present (used for the
    /// account fingerprint since email/org names aren't readable).
    #[serde(default)]
    pub cwd: Option<String>,
    /// RFC 3339 timestamp of the scan that last saw this file.
    pub last_seen_at: String,
    /// Transcript this copy points at, and the ones it continued from: a
    /// copy listing another copy's transcript as prior is certainly newer.
    #[serde(default)]
    pub cli_session_id: Option<String>,
    #[serde(default)]
    pub prior_cli_session_ids: Vec<String>,
    /// `lastActivityAt` from the JSON (ms), when present.
    #[serde(default)]
    pub last_activity: Option<i64>,
}

/// A `deleted_<chat>` marker: Claude writes one (containing the deletion
/// time) in place of `local_<chat>.json` when a conversation is deleted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tombstone {
    pub account_root: PathBuf,
    /// Absolute path of the marker file.
    pub file_path: PathBuf,
    /// The conversation this marker stands for, as its file name
    /// (`local_<chat>.json`) — the same sync key as `FileEntry::chat`.
    pub session_path: PathBuf,
    pub mtime: i64,
}

/// The persisted view of every discovered tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub generated_at: String,
    pub sessions_root: PathBuf,
    pub account_roots: Vec<PathBuf>,
    pub entries: Vec<FileEntry>,
    #[serde(default)]
    pub tombstones: Vec<Tombstone>,
    /// Per conversation: the version every account holding it last agreed
    /// on (content hash). Lets sync tell a real edit from a file whose
    /// mtime merely moved (Claude rewrites a chat just by opening it).
    #[serde(default)]
    pub base: std::collections::BTreeMap<PathBuf, String>,
    /// Per conversation: versions that were agreed on earlier and have since
    /// been replaced. A copy that goes back to one of these is stale (Claude
    /// wrote an old in-memory copy back), never a new edit.
    #[serde(default)]
    pub history: std::collections::BTreeMap<PathBuf, Vec<String>>,
    /// Every organization folder (`<account>/<org>`) found, including empty
    /// ones: each of them should hold every conversation.
    #[serde(default)]
    pub org_dirs: Vec<PathBuf>,
    /// `scan::HASH_VERSION` the hashes above were computed with.
    #[serde(default)]
    pub hash_version: u32,
    /// Files skipped by this scan because they were still being written;
    /// a watcher should rescan shortly even if no new event arrives.
    #[serde(skip)]
    pub skipped_recent: usize,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        Ok(serde_json::from_str(&raw)
            .with_context(|| format!("parsing manifest {}", path.display()))?)
    }

    pub fn load_or_default(path: &Path) -> Manifest {
        if path.exists() {
            Manifest::load(path).unwrap_or_default()
        } else {
            Manifest::default()
        }
    }

    /// Atomic save: temp file + rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        replace_file(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Entries grouped by account root, in the order of `account_roots`.

    pub fn entries_for(&self, root: &Path) -> Vec<&FileEntry> {
        self.entries.iter().filter(|e| e.account_root == root).collect()
    }
}

impl FileEntry {
    /// The sync key: the conversation's file name (`local_<chat>.json`). The
    /// same conversation is the same file in every account and organization.
    pub fn chat(&self) -> PathBuf {
        PathBuf::from(self.file_path.file_name().unwrap_or_default())
    }

    /// True if this copy continued from `other`'s transcript.
    pub fn continues(&self, other: &FileEntry) -> bool {
        other.cli_session_id.as_ref().is_some_and(|c| {
            self.cli_session_id.as_ref() != Some(c) && self.prior_cli_session_ids.contains(c)
        })
    }

    /// The organization folder holding this file.
    pub fn folder(&self) -> &Path {
        self.file_path.parent().unwrap_or(&self.account_root)
    }

    /// The organization folder this session lives in: the first path
    /// component below the account root (Claude nests
    /// `<account>/<organization>/local_<chat>.json`). None if the file sits
    /// directly under the account root.
    pub fn organization(&self) -> Option<&str> {
        self.relative_path
            .components()
            .next()
            .map(|c| c.as_os_str())
            .and_then(|s| s.to_str())
            .filter(|_| self.relative_path.components().count() > 1)
    }
}

/// Rename `tmp` over `dest`, replacing it if present. On unix this is a
/// plain atomic rename; Windows refuses to rename over an existing file,
/// so the old file is removed first (the data is already safe in `tmp`).
pub fn replace_file(tmp: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    std::fs::rename(tmp, dest)
}
