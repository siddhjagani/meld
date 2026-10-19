use crate::config::Config;
use crate::errors::Result;
use crate::hash::sha256_file;
use crate::logging;
use crate::manifest::{FileEntry, Manifest, Tombstone};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use std::collections::{BTreeMap, BTreeSet};

/// Files modified within this window are considered "possibly still being
/// written" and skipped for one scan cycle.
const WRITE_QUIET_SECS: i64 = 2;

/// Keep at most this many superseded versions per conversation.
const MAX_HISTORY: usize = 50;

/// Walk every organization folder of every account and build a fresh
/// manifest. Hashes are reused from `previous` when size+mtime are
/// unchanged, so rescans are cheap.
pub fn scan(cfg: &Config, roots: &[PathBuf], previous: &Manifest) -> Result<Manifest> {
    let now = Utc::now();
    let mut entries = Vec::new();
    let mut tombstones = Vec::new();
    let mut all_org_dirs = Vec::new();
    let mut skipped_recent = 0usize;
    // Conversations with a file we could not look at yet (still being
    // written); their agreed version must not be updated this round.
    let mut unsettled: BTreeSet<PathBuf> = BTreeSet::new();

    let same_hashing = previous.hash_version == HASH_VERSION;
    for root in roots {
        for dir in org_dirs(root) {
            let items = match std::fs::read_dir(&dir) {
                Ok(i) => i,
                Err(e) => {
                    logging::warn(&format!("skipping unreadable folder {}: {e}", dir.display()));
                    continue;
                }
            };
            all_org_dirs.push(dir.clone());
            for item in items.flatten() {
                let path = item.path();
                if !item.file_type().is_ok_and(|t| t.is_file()) || cfg.is_ignored(&path) {
                    continue;
                }
                let Some(kind) = file_kind(&path) else {
                    continue;
                };
                let meta = match item.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        logging::warn(&format!("stat failed for {}: {e}", path.display()));
                        continue;
                    }
                };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let name = PathBuf::from(item.file_name());

                if kind == FileKind::Tombstone {
                    if let Some(session_path) = tombstone_session_path(&name) {
                        tombstones.push(Tombstone {
                            account_root: root.clone(),
                            file_path: path.clone(),
                            session_path,
                            mtime,
                        });
                    }
                    continue;
                }

                if now.timestamp() - mtime < WRITE_QUIET_SECS {
                    skipped_recent += 1;
                    unsettled.insert(name);
                    logging::verbose(&format!(
                        "skipping (recently written): {}",
                        path.display()
                    ));
                    continue;
                }

                let size = meta.len();
                let relative_path = path.strip_prefix(root)?.to_path_buf();

                // Reuse hashes if the file looks unchanged since the last scan.
                let prev = previous.entries.iter().filter(|_| same_hashing).find(|e| {
                    e.file_path == path
                        && e.size == size
                        && e.mtime == mtime
                        && !e.content_hash.is_empty()
                });
                let (sha256, reused_content_hash) = match prev {
                    Some(p) => (p.sha256.clone(), Some(p.content_hash.clone())),
                    None => match sha256_file(&path) {
                        Ok(h) => (h, None),
                        Err(e) => {
                            logging::warn(&format!("hash failed for {}: {e}", path.display()));
                            continue;
                        }
                    },
                };

                let meta = read_session_meta(&path);
                let content_hash = reused_content_hash
                    .or_else(|| normalized_hash(&path))
                    .unwrap_or_else(|| sha256.clone());
                entries.push(FileEntry {
                    account_root: root.clone(),
                    file_path: path,
                    relative_path,
                    sha256,
                    content_hash,
                    size,
                    mtime,
                    session_id: meta.session_id,
                    title: meta.title,
                    cwd: meta.cwd,
                    last_seen_at: now.to_rfc3339(),
                    cli_session_id: meta.cli_session_id,
                    prior_cli_session_ids: meta.prior_cli_session_ids,
                    last_activity: meta.last_activity,
                });
            }
        }
    }

    let empty = Manifest::default();
    let earlier = if same_hashing { previous } else { &empty };
    let (base, history) = agreed_versions(&entries, &unsettled, &earlier.base, &earlier.history);

    Ok(Manifest {
        generated_at: now.to_rfc3339(),
        sessions_root: cfg.sessions_root.clone(),
        account_roots: roots.to_vec(),
        entries,
        tombstones,
        base,
        history,
        org_dirs: all_org_dirs,
        hash_version: HASH_VERSION,
        skipped_recent,
    })
}

/// Organization folders of one account: the UUID-named folders directly
/// under it, the shape Claude uses. Other folders (made by hand or by other
/// tools) are left alone.
pub fn org_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(items) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut dirs: Vec<PathBuf> = items
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name().to_str().is_some_and(is_org_name))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// Claude names organization folders by UUID: 36 hex digits and dashes
/// (the same check the desktop app uses).
pub fn is_org_name(name: &str) -> bool {
    name.len() == 36 && name.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// For each conversation, the content hash every copy agrees on right now
/// (two or more copies must exist). While the copies disagree, only one
/// remains, or one is still being written, the previously agreed version is
/// kept, so a later diff can tell which copy actually changed. Versions that
/// stop being the agreed one go to `history`. Conversations that no longer
/// exist are dropped.
fn agreed_versions(
    entries: &[FileEntry],
    unsettled: &BTreeSet<PathBuf>,
    previous: &BTreeMap<PathBuf, String>,
    previous_history: &BTreeMap<PathBuf, Vec<String>>,
) -> (BTreeMap<PathBuf, String>, BTreeMap<PathBuf, Vec<String>>) {
    let mut by_chat: BTreeMap<PathBuf, Vec<&str>> = BTreeMap::new();
    for e in entries {
        by_chat.entry(e.chat()).or_default().push(&e.content_hash);
    }
    let mut base = BTreeMap::new();
    let mut history = BTreeMap::new();
    for (chat, hashes) in by_chat {
        let agree = !unsettled.contains(&chat) && hashes.iter().all(|h| *h == hashes[0]);
        let old = previous.get(&chat);
        // A lone copy proves nothing: it may be the only survivor of a
        // deletion with an edit in it. It only seeds a conversation that has
        // no recorded version yet.
        let new = if agree && (hashes.len() > 1 || old.is_none()) {
            Some(hashes[0].to_string())
        } else {
            old.cloned()
        };
        let mut past = previous_history.get(&chat).cloned().unwrap_or_default();
        if let (Some(o), Some(n)) = (old, &new) {
            if o != n && !past.contains(o) {
                past.push(o.clone());
                if past.len() > MAX_HISTORY {
                    past.remove(0);
                }
            }
        }
        if let Some(n) = &new {
            past.retain(|h| h != n); // everyone went back to it: it is current again
        }
        if !past.is_empty() {
            history.insert(chat.clone(), past);
        }
        if let Some(n) = new {
            base.insert(chat, n);
        }
    }
    (base, history)
}

#[derive(Debug, PartialEq)]
enum FileKind {
    Session,
    Tombstone,
}

/// Only conversation files (`local_<chat>.json`) and Claude's deletion
/// markers (`deleted_<chat>`, no extension) take part in sync. Other
/// per-account files such as `scheduled-tasks.json` stay where they are.
fn file_kind(path: &Path) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with("local_") && name.ends_with(".json") {
        Some(FileKind::Session)
    } else if name.starts_with("deleted_") && path.extension().is_none() {
        Some(FileKind::Tombstone)
    } else {
        None
    }
}

/// `deleted_<chat>` -> `local_<chat>.json` (keeps any leading folders)
pub fn tombstone_session_path(rel: &Path) -> Option<PathBuf> {
    let name = rel.file_name()?.to_str()?;
    let id = name.strip_prefix("deleted_")?;
    Some(rel.with_file_name(format!("local_{id}.json")))
}

/// `local_<chat>.json` -> `deleted_<chat>` (keeps any leading folders)
pub fn session_tombstone_path(rel: &Path) -> Option<PathBuf> {
    let name = rel.file_name()?.to_str()?;
    let id = name.strip_prefix("local_")?.strip_suffix(".json")?;
    Some(rel.with_file_name(format!("deleted_{id}")))
}

/// Bookkeeping fields Claude Code rewrites every time a session is merely
/// opened or focused. Differences in these fields do not make two copies of
/// a conversation "different" for sync purposes.
/// Fields that belong to one account/organization or one running process,
/// not to the conversation: that org's connectors and tools, a "usage
/// limit reached" error from that account, remote-control and local process
/// handles. They never make copies "different", and an update keeps the
/// target's own values (see `sync::merged_update`).
pub const LOCAL_KEYS: &[&str] = &[
    "remoteMcpServersConfig",
    "enabledMcpTools",
    "error",
    "errorRows",
    "priorErrorMark",
    "bridgeSessionIds",
    "remoteControlAutoEligible",
    "remoteControlUserEnabled",
    "steeredByRemoteClient",
    "sshRemoteProcessId",
    "sshReattach",
    "chromeTabGroupId",
    "terminalClaudeTabOrdinal",
    "lastSpawnRootDetected",
    "interruptedByQuitAt",
];

/// Bumped whenever `normalized_hash` changes, so hashes, agreed versions
/// and history saved by an older meld are recomputed instead of reused.
pub const HASH_VERSION: u32 = 2;

const VOLATILE_KEYS: &[&str] = &[
    "lastFocusedAt",
    "lastActivityAt",
    "completedTurns",
    "errorAt",
];

/// Hash of the session JSON with volatile keys stripped and keys sorted
/// (serde_json maps preserve insertion order of the source, so re-serializing
/// a filtered BTreeMap gives a canonical form). None if the file isn't a
/// JSON object.
fn normalized_hash(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let obj = v.as_object()?;
    let filtered: std::collections::BTreeMap<&String, &serde_json::Value> = obj
        .iter()
        .filter(|(k, _)| !VOLATILE_KEYS.contains(&k.as_str()) && !LOCAL_KEYS.contains(&k.as_str()))
        .collect();
    let canonical = serde_json::to_string(&filtered).ok()?;
    use sha2::{Digest, Sha256};
    Some(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}

/// Lightweight session metadata read from the JSON.
pub struct SessionMeta {
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub cli_session_id: Option<String>,
    pub prior_cli_session_ids: Vec<String>,
    pub last_activity: Option<i64>,
}

/// Session id, title and cwd from JSON content. The id falls back to the
/// `local_<uuid>.json` filename convention when the field is absent.
pub fn read_session_meta(path: &Path) -> SessionMeta {
    let mut id = None;
    let mut title = None;
    let mut cwd = None;
    let mut cli_session_id = None;
    let mut prior_cli_session_ids = Vec::new();
    let mut last_activity = None;
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            id = v.get("sessionId").and_then(|s| s.as_str()).map(String::from);
            let str_field = |key: &str| {
                v.get(key)
                    .and_then(|s| s.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(String::from)
            };
            title = str_field("title");
            cwd = str_field("cwd");
            cli_session_id = str_field("cliSessionId");
            prior_cli_session_ids = v
                .get("priorCliSessionIds")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default();
            last_activity = v.get("lastActivityAt").and_then(|n| n.as_i64());
        }
    }
    if id.is_none() {
        id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("local_"))
            .map(String::from);
    }
    SessionMeta {
        session_id: id,
        title,
        cwd,
        cli_session_id,
        prior_cli_session_ids,
        last_activity,
    }
}
