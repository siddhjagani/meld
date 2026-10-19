use crate::config::Config;
use crate::errors::Result;
use crate::hash::sha256_file;
use crate::logging;
use crate::manifest::{FileEntry, Manifest, Tombstone};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

/// Files modified within this window are considered "possibly still being
/// written" and skipped for one scan cycle.
const WRITE_QUIET_SECS: i64 = 2;

/// Walk every account root and build a fresh manifest. Hashes are reused
/// from `previous` when size+mtime are unchanged, so rescans are cheap.
pub fn scan(cfg: &Config, roots: &[PathBuf], previous: &Manifest) -> Result<Manifest> {
    let now = Utc::now();
    let mut entries = Vec::new();
    let mut tombstones = Vec::new();
    let mut skipped_recent = 0usize;
    // Conversations with a file we could not look at yet (still being
    // written); their agreed version must not be updated this round.
    let mut unsettled: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();

    for root in roots {
        for item in WalkDir::new(root).follow_links(false) {
            let item = match item {
                Ok(i) => i,
                Err(e) => {
                    logging::warn(&format!("skipping unreadable entry: {e}"));
                    continue;
                }
            };
            if !item.file_type().is_file() {
                continue;
            }
            let path = item.path();
            if cfg.is_ignored(path) {
                continue;
            }
            let Some(kind) = file_kind(path) else {
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

            if kind == FileKind::Tombstone {
                let rel = path.strip_prefix(root)?;
                if let Some(session_path) = tombstone_session_path(rel) {
                    tombstones.push(Tombstone {
                        account_root: root.clone(),
                        file_path: path.to_path_buf(),
                        session_path,
                        mtime,
                    });
                }
                continue;
            }

            if now.timestamp() - mtime < WRITE_QUIET_SECS {
                skipped_recent += 1;
                if let Ok(rel) = path.strip_prefix(root) {
                    unsettled.insert(rel.to_path_buf());
                }
                logging::verbose(&format!(
                    "skipping (recently written): {}",
                    path.display()
                ));
                continue;
            }

            let size = meta.len();
            let relative_path = path.strip_prefix(root)?.to_path_buf();

            // Reuse hashes if the file looks unchanged since the last scan.
            let prev = previous.entries.iter().find(|e| {
                e.file_path == path
                    && e.size == size
                    && e.mtime == mtime
                    && !e.content_hash.is_empty()
            });
            let (sha256, reused_content_hash) = match prev {
                Some(p) => (p.sha256.clone(), Some(p.content_hash.clone())),
                None => match sha256_file(path) {
                    Ok(h) => (h, None),
                    Err(e) => {
                        logging::warn(&format!("hash failed for {}: {e}", path.display()));
                        continue;
                    }
                },
            };

            let meta = read_session_meta(path);
            let content_hash = reused_content_hash
                .or_else(|| normalized_hash(path))
                .unwrap_or_else(|| sha256.clone());
            entries.push(FileEntry {
                account_root: root.clone(),
                file_path: path.to_path_buf(),
                relative_path,
                sha256,
                content_hash,
                size,
                mtime,
                session_id: meta.session_id,
                title: meta.title,
                cwd: meta.cwd,
                last_seen_at: now.to_rfc3339(),
            });
        }
    }

    let base = agreed_versions(&entries, &unsettled, &previous.base);

    Ok(Manifest {
        generated_at: now.to_rfc3339(),
        sessions_root: cfg.sessions_root.clone(),
        account_roots: roots.to_vec(),
        entries,
        tombstones,
        base,
        skipped_recent,
    })
}

/// For each conversation, the content hash every account holding it agrees
/// on right now (two or more accounts must hold it). While the copies
/// disagree, only one remains, or one is still being written, the previously
/// agreed version is kept, so a later diff can tell which
/// copy actually changed. Conversations that no longer exist are dropped.
fn agreed_versions(
    entries: &[FileEntry],
    unsettled: &std::collections::BTreeSet<PathBuf>,
    previous: &std::collections::BTreeMap<PathBuf, String>,
) -> std::collections::BTreeMap<PathBuf, String> {
    let mut by_rel: std::collections::BTreeMap<&PathBuf, Vec<&str>> = std::collections::BTreeMap::new();
    for e in entries {
        by_rel.entry(&e.relative_path).or_default().push(&e.content_hash);
    }
    let mut base = std::collections::BTreeMap::new();
    for (rel, hashes) in by_rel {
        let agree = !unsettled.contains(rel) && hashes.iter().all(|h| *h == hashes[0]);
        // A lone copy proves nothing: it may be the only survivor of a
        // deletion with an edit in it. It only seeds a conversation that has
        // no recorded version yet.
        if agree && (hashes.len() > 1 || !previous.contains_key(rel)) {
            base.insert(rel.clone(), hashes[0].to_string());
        } else if let Some(b) = previous.get(rel) {
            base.insert(rel.clone(), b.clone());
        }
    }
    base
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

/// `<org>/deleted_<chat>` -> `<org>/local_<chat>.json`
pub fn tombstone_session_path(rel: &Path) -> Option<PathBuf> {
    let name = rel.file_name()?.to_str()?;
    let id = name.strip_prefix("deleted_")?;
    Some(rel.with_file_name(format!("local_{id}.json")))
}

/// `<org>/local_<chat>.json` -> `<org>/deleted_<chat>`
pub fn session_tombstone_path(rel: &Path) -> Option<PathBuf> {
    let name = rel.file_name()?.to_str()?;
    let id = name.strip_prefix("local_")?.strip_suffix(".json")?;
    Some(rel.with_file_name(format!("deleted_{id}")))
}

/// Bookkeeping fields Claude Code rewrites every time a session is merely
/// opened or focused. Differences in these fields do not make two copies of
/// a conversation "different" for sync purposes.
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
        .filter(|(k, _)| !VOLATILE_KEYS.contains(&k.as_str()))
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
}

/// Session id, title and cwd from JSON content. The id falls back to the
/// `local_<uuid>.json` filename convention when the field is absent.
pub fn read_session_meta(path: &Path) -> SessionMeta {
    let mut id = None;
    let mut title = None;
    let mut cwd = None;
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
        }
    }
    if id.is_none() {
        id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("local_"))
            .map(String::from);
    }
    SessionMeta { session_id: id, title, cwd }
}
