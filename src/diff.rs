use crate::config::{DeletedChats, Policy};
use crate::manifest::{FileEntry, Manifest, Tombstone};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A file that should be copied into an account tree that lacks it.
#[derive(Debug, Clone)]
pub struct CopyAction {
    pub source: FileEntry,
    pub dest_root: PathBuf,
    pub dest_path: PathBuf,
}

/// An older copy of a conversation that the newest copy should replace.
#[derive(Debug, Clone)]
pub struct UpdateAction {
    pub source: FileEntry,
    pub target: FileEntry,
}

/// A conversation that was deleted in another account and should be
/// deleted here too, the way Claude does it: write the `deleted_<chat>`
/// marker, then remove `local_<chat>.json`.
#[derive(Debug, Clone)]
pub struct DeleteAction {
    pub target: FileEntry,
    pub tombstone: Tombstone,
    pub tombstone_dest: PathBuf,
}

/// Same relative path exists in two trees with different content and no
/// copy can be picked as the newest. Never resolved silently — surfaced to
/// the user.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub relative_path: PathBuf,
    pub variants: Vec<FileEntry>,
}

#[derive(Debug, Default)]
pub struct DiffReport {
    pub copies: Vec<CopyAction>,
    pub updates: Vec<UpdateAction>,
    pub deletes: Vec<DeleteAction>,
    pub conflicts: Vec<Conflict>,
    /// relative_path -> roots that already have it (for status output).
    pub coverage: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl DiffReport {
    /// Number of file changes a sync would make.
    pub fn pending(&self) -> usize {
        self.copies.len() + self.updates.len() + self.deletes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
}

/// How the canonical copy of a diverged conversation was chosen.
enum Verdict {
    /// The canonical copy is known to be the latest: it is the only real
    /// edit since the copies last agreed, or the agreed version itself when
    /// the other copies only went back to older versions.
    Known,
    /// No agreed version on record; transcript lineage, then the latest
    /// activity and file time decide.
    ByMtime,
    /// Several copies were edited differently: never pick one.
    Conflict,
}

/// Ordering for "most recently used" when no history is known.
fn recency(e: &FileEntry) -> (i64, i64) {
    (e.last_activity.unwrap_or(0), e.mtime)
}

/// Several copies were edited since they last agreed. If they all point at
/// the same transcript (or one continued the others' transcripts), they are
/// one conversation saved at different moments, e.g. by two Claude windows
/// signed in to different accounts: the most recent wins. Otherwise None.
fn latest_of_one_conversation<'a>(edited: &[&'a FileEntry]) -> Option<&'a FileEntry> {
    let head = edited.iter().copied().find(|e| {
        e.cli_session_id.is_some()
            && edited.iter().all(|o| o.cli_session_id == e.cli_session_id || e.continues(o))
    })?;
    edited
        .iter()
        .copied()
        .filter(|e| e.cli_session_id == head.cli_session_id)
        .max_by_key(|e| recency(e))
}

/// True if `e` holds content no copy has agreed on before: a real edit made
/// since the last agreement, not an older version written back (Claude
/// saves its in-memory copy, which can be one meld already replaced).
fn is_new_edit(e: &FileEntry, base: Option<&String>, past: Option<&Vec<String>>) -> bool {
    base.is_some_and(|b| &e.content_hash != b)
        && !past.is_some_and(|p| p.contains(&e.content_hash))
}

/// Mirror diff: every organization folder of every account should end up
/// holding every conversation, in the same version. A conversation is
/// identified by its file name (`local_<chat>.json`), wherever it lives.
///
/// Policy, in order:
/// 1. a conversation with a `deleted_<chat>` marker: with
///    `delete_everywhere` it is deleted in every folder that has it; with
///    `stay_deleted_here` it is not copied back into the folder holding the
///    marker; with `come_back` (default) the marker is ignored
/// 2. folder missing the conversation → copy the canonical version
/// 3. copies with different content → the canonical copy replaces the others
///    (`propagate_updates`); when no copy can be picked, or updates are off,
///    the divergence is reported as a conflict and nothing is overwritten.
pub fn compute(manifest: &Manifest, policy: Policy) -> DiffReport {
    let mut by_chat: BTreeMap<PathBuf, Vec<&FileEntry>> = BTreeMap::new();
    for e in &manifest.entries {
        by_chat.entry(e.chat()).or_default().push(e);
    }

    let mut markers: BTreeMap<&PathBuf, Vec<&Tombstone>> = BTreeMap::new();
    for t in &manifest.tombstones {
        markers.entry(&t.session_path).or_default().push(t);
    }

    // Accounts with no organization folder yet get the source's folder name.
    let bare_accounts: Vec<&PathBuf> = manifest
        .account_roots
        .iter()
        .filter(|r| !manifest.org_dirs.iter().any(|d| d.parent() == Some(r.as_path())))
        .collect();

    let mut report = DiffReport::default();

    for (chat, entries) in &by_chat {
        let mut have_roots: Vec<PathBuf> =
            entries.iter().map(|e| e.account_root.clone()).collect();
        have_roots.dedup();
        report.coverage.insert(chat.clone(), have_roots);

        let base = manifest.base.get(chat);
        let past = manifest.history.get(chat);
        let chat_markers = markers.get(chat);

        if policy.deletes == DeletedChats::DeleteEverywhere {
            if let (Some(ms), Some(marker)) = (chat_markers, crate::scan::session_tombstone_path(chat)) {
                // A copy edited since the copies last agreed holds work the
                // deleter never saw. Keep every copy and surface it.
                if entries.iter().any(|e| is_new_edit(e, base, past)) {
                    report.conflicts.push(Conflict {
                        relative_path: chat.clone(),
                        variants: entries.iter().map(|e| (*e).clone()).collect(),
                    });
                    continue;
                }
                let newest_marker = ms.iter().max_by_key(|t| t.mtime).expect("non-empty");
                for e in entries {
                    report.deletes.push(DeleteAction {
                        target: (*e).clone(),
                        tombstone: (*newest_marker).clone(),
                        tombstone_dest: e.folder().join(&marker),
                    });
                }
                continue;
            }
        }

        // Without history: a copy that continued another copy's transcript
        // is newer; otherwise the latest activity, then the latest file time.
        let newest = entries
            .iter()
            .copied()
            .filter(|e| !entries.iter().any(|o| o.continues(e)))
            .max_by_key(|e| recency(e))
            .or_else(|| entries.iter().copied().max_by_key(|e| recency(e)))
            .expect("non-empty group");

        // Which copy is the latest version? If we know the version the
        // copies last agreed on, the copies holding new content are the real
        // edits (an unchanged copy with a fresher mtime is not one, and
        // neither is a copy that went back to an older version). Without
        // that history, fall back to the newest mtime.
        let (canonical, verdict) = match base {
            Some(b) => {
                let edited: Vec<&FileEntry> = entries
                    .iter()
                    .copied()
                    .filter(|e| is_new_edit(e, base, past))
                    .collect();
                match edited.first() {
                    Some(f) if edited.iter().all(|e| e.content_hash == f.content_hash) => (
                        edited.iter().copied().max_by_key(|e| (e.mtime, e.size)).unwrap(),
                        Verdict::Known,
                    ),
                    Some(_) => match latest_of_one_conversation(&edited) {
                        Some(latest) => (latest, Verdict::Known),
                        None => (newest, Verdict::Conflict), // edited differently in several places
                    },
                    // No new edits: copies that differ went back to an older
                    // version, so the agreed version goes back over them.
                    None => match entries.iter().copied().find(|e| &e.content_hash == b) {
                        Some(current) => (current, Verdict::Known),
                        None => (newest, Verdict::Conflict),
                    },
                }
            }
            None => (newest, Verdict::ByMtime),
        };

        // content_hash ignores volatile bookkeeping fields, so two copies of
        // a conversation that only differ in "last opened" timestamps are
        // treated as identical.
        let stale: Vec<&FileEntry> = entries
            .iter()
            .copied()
            .filter(|e| e.content_hash != canonical.content_hash)
            .collect();
        if !stale.is_empty() {
            // Without history, only overwrite when the canonical copy is
            // strictly newer than every copy it would replace; a tie means
            // we can't tell which version is the latest.
            let replace = match verdict {
                Verdict::Known => true,
                Verdict::ByMtime => stale
                    .iter()
                    .all(|e| canonical.continues(e) || (!e.continues(canonical) && recency(e) < recency(canonical))),
                Verdict::Conflict => false,
            };
            if policy.propagate_updates && replace {
                for e in stale {
                    report.updates.push(UpdateAction {
                        source: (*canonical).clone(),
                        target: e.clone(),
                    });
                }
            } else {
                report.conflicts.push(Conflict {
                    relative_path: chat.clone(),
                    variants: entries.iter().map(|e| (*e).clone()).collect(),
                });
            }
        }

        for dir in &manifest.org_dirs {
            if entries.iter().any(|e| e.folder() == dir) {
                continue;
            }
            let deleted_here = chat_markers
                .is_some_and(|ms| ms.iter().any(|t| t.file_path.parent() == Some(dir.as_path())));
            if deleted_here && policy.deletes == DeletedChats::StayDeletedHere {
                continue;
            }
            report.copies.push(CopyAction {
                source: (*canonical).clone(),
                dest_root: dir.parent().unwrap_or(dir).to_path_buf(),
                dest_path: dir.join(chat),
            });
        }
        for root in &bare_accounts {
            report.copies.push(CopyAction {
                source: (*canonical).clone(),
                dest_root: (*root).clone(),
                dest_path: root.join(&canonical.relative_path),
            });
        }
    }

    report
}
