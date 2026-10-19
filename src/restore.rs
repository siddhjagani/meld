use crate::backup;
use crate::config::{Config, DeletedChats};
use crate::errors::{MeldError, Result};
use crate::{discover, logging, scan};
use anyhow::Context;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Restore files from the newest snapshot (or a named one).
///
/// - Files created after the snapshot are kept.
/// - Before anything is written, the files about to be replaced or removed
///   are saved to a new snapshot, so a restore can itself be undone.
/// - With `propagate_updates`, restored copies are stamped as the newest
///   version, so the next sync spreads them to every account instead of
///   replacing them.
/// - With `deleted_chats = "delete_everywhere"`, a conversation brought back is un-deleted
///   everywhere: deletion markers for it that are not part of the snapshot
///   are removed from every account.
pub fn restore(cfg: &Config, name: Option<&str>, dry_run: bool) -> Result<usize> {
    let snaps = backup::list_snapshots(cfg)?;
    let snap = match name {
        Some(n) => snaps
            .iter()
            .find(|s| s.file_name().and_then(|f| f.to_str()) == Some(n))
            .cloned()
            .ok_or(MeldError::NoSnapshot)?,
        None => snaps.last().cloned().ok_or(MeldError::NoSnapshot)?,
    };
    let label = snap
        .file_name()
        .and_then(|f| f.to_str())
        .and_then(pretty_stamp)
        .unwrap_or_else(|| snap.display().to_string());
    logging::info(&format!("Restoring your sessions from the backup taken {label}..."));
    logging::verbose(&format!("backup location: {}", snap.display()));

    // (file in snapshot, where it goes back to), skipping files that are
    // already identical.
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Snapshot contents relative to their account root, to tell whether a
    // deletion marker was part of the snapshot.
    let mut in_snapshot: BTreeSet<PathBuf> = BTreeSet::new();
    for item in WalkDir::new(&snap).follow_links(false) {
        let item = item?;
        if !item.file_type().is_file() {
            continue;
        }
        let rel = item.path().strip_prefix(&snap)?;
        if let Some(name) = rel.file_name() {
            in_snapshot.insert(PathBuf::from(name));
        }
        let target = cfg.sessions_root.join(rel);
        if same_bytes(item.path(), &target) {
            continue;
        }
        plan.push((item.path().to_path_buf(), target));
    }

    let roots = discover::discover_account_roots(cfg)?;
    let everywhere = cfg.policy().deletes == DeletedChats::DeleteEverywhere;
    let mut lift: Vec<PathBuf> = Vec::new();
    for (_, target) in plan.iter().filter(|_| everywhere) {
        let Some(marker) = target.file_name().and_then(|n| scan::session_tombstone_path(Path::new(n))) else {
            continue;
        };
        if in_snapshot.contains(&marker) {
            continue;
        }
        for dir in roots.iter().flat_map(|r| scan::org_dirs(r)) {
            let m = dir.join(&marker);
            if m.exists() && !lift.contains(&m) {
                lift.push(m);
            }
        }
    }

    if dry_run {
        for (_, target) in &plan {
            logging::verbose(&format!("would restore {}", target.display()));
        }
        for m in &lift {
            logging::verbose(&format!("would remove deletion marker {}", m.display()));
        }
        logging::info(&format!(
            "This would put back {} files{}. Nothing has been changed.",
            plan.len(),
            undelete_note(lift.len())
        ));
        return Ok(plan.len());
    }

    let at_risk: Vec<PathBuf> = plan
        .iter()
        .map(|(_, t)| t.clone())
        .filter(|t| t.exists())
        .chain(lift.iter().cloned())
        .collect();
    let undo = if at_risk.is_empty() {
        None
    } else {
        Some(backup::snapshot_files(cfg, &at_risk, Some(&snap))?)
    };

    let now = filetime::FileTime::now();
    for (src, target) in &plan {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Atomic write: copy to temp, then rename over the target.
        let tmp = target.with_extension("meld-restore-tmp");
        std::fs::copy(src, &tmp)
            .with_context(|| format!("restoring {}", target.display()))?;
        crate::manifest::replace_file(&tmp, target)?;
        if cfg.propagate_updates {
            let _ = filetime::set_file_mtime(target, now);
        }
        logging::verbose(&format!("restored {}", target.display()));
    }
    for m in &lift {
        std::fs::remove_file(m)
            .with_context(|| format!("removing deletion marker {}", m.display()))?;
        logging::verbose(&format!("removed deletion marker {}", m.display()));
    }

    logging::info(&format!(
        "Done. {} files put back{}. Conversations created after the backup were kept.",
        plan.len(),
        undelete_note(lift.len())
    ));
    if let Some(u) = undo.as_ref().and_then(|u| u.file_name()).and_then(|n| n.to_str()) {
        logging::info(&format!("To undo this restore: meld restore --snapshot {u}"));
    }
    Ok(plan.len())
}

/// Print every snapshot, oldest first, with its file count.
pub fn list(cfg: &Config) -> Result<()> {
    let snaps = backup::list_snapshots(cfg)?;
    if snaps.is_empty() {
        println!("No backups yet.");
        return Ok(());
    }
    for s in &snaps {
        let name = s.file_name().and_then(|f| f.to_str()).unwrap_or("?");
        let files = WalkDir::new(s)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count();
        println!(
            "{name}  {}  ({files} files)",
            pretty_stamp(name).unwrap_or_default()
        );
    }
    println!("\nRestore one with `meld restore --snapshot <name>`.");
    Ok(())
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn undelete_note(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " and lifted 1 deletion marker".into(),
        n => format!(" and lifted {n} deletion markers"),
    }
}

/// "20260707T143626010Z" -> "on 2026-07-07 at 14:36"
fn pretty_stamp(name: &str) -> Option<String> {
    let (date, rest) = name.split_once('T')?;
    if date.len() != 8 || rest.len() < 4 {
        return None;
    }
    Some(format!(
        "on {}-{}-{} at {}:{}",
        &date[..4],
        &date[4..6],
        &date[6..8],
        &rest[..2],
        &rest[2..4]
    ))
}
