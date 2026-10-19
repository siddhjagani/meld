use crate::backup;
use crate::config::Config;
use crate::discover;
use crate::diff::DiffReport;
use crate::errors::Result;
use crate::logging;
use crate::manifest::{replace_file, FileEntry};
use anyhow::Context;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub copied: usize,
    pub updated: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub conflicts: usize,
    pub snapshot: Option<PathBuf>,
}

/// Apply a diff: copy missing conversations, replace older copies with the
/// newest one, and delete conversations that were deleted elsewhere.
///
/// Safety invariants:
/// - a snapshot is saved to ~/.meld/backups before the first write (unless
///   `no_backup`): every file that is replaced or removed, or all trees for
///   a copy-only sync, so `meld restore` undoes it
/// - a target that changed since the scan is skipped, not clobbered; the
///   next sync looks at it again
/// - writes are temp-file + fsync + rename, so a crash can't leave a
///   half-written session file where Claude Code will read it
pub fn apply(cfg: &Config, report: &DiffReport, dry_run: bool, no_backup: bool) -> Result<SyncOutcome> {
    let mut out = SyncOutcome {
        conflicts: report.conflicts.len(),
        ..Default::default()
    };

    if report.is_empty() {
        logging::info("All accounts are already up to date.");
        return Ok(out);
    }

    if dry_run {
        logging::info(&format!(
            "This would change {}. Nothing has been changed.",
            files(report.pending())
        ));
        print_plan(report);
        logging::info("Run `meld sync` to apply.");
        out.copied = report.copies.len();
        out.updated = report.updates.len();
        out.deleted = report.deletes.len();
        return Ok(out);
    }

    // Updates and deletes replace or remove files, so exactly those targets
    // are backed up (small, frequent under full sync). A copy-only sync
    // keeps the full snapshot of every tree.
    let at_risk: Vec<PathBuf> = report
        .updates
        .iter()
        .map(|u| u.target.file_path.clone())
        .chain(report.deletes.iter().map(|d| d.target.file_path.clone()))
        .collect();
    if !no_backup {
        out.snapshot = Some(if at_risk.is_empty() {
            logging::info("Saving a backup of your sessions first...");
            backup::snapshot(cfg, &discover::discover_account_roots(cfg)?)?
        } else {
            backup::snapshot_files(cfg, &at_risk, None)?
        });
    }

    for c in &report.copies {
        // Re-check at write time: the file may have appeared since the scan.
        if c.dest_path.exists() {
            logging::verbose(&format!(
                "skipping, destination now exists: {}",
                c.dest_path.display()
            ));
            out.skipped += 1;
            continue;
        }
        write_atomic(&c.dest_path, &read(&c.source.file_path)?, c.source.mtime)?;
        logging::verbose(&format!(
            "copied {} -> {}",
            c.source.relative_path.display(),
            c.dest_path.display()
        ));
        out.copied += 1;
    }

    for u in &report.updates {
        if changed_since_scan(&u.target) {
            logging::verbose(&format!(
                "skipping, changed since scan: {}",
                u.target.file_path.display()
            ));
            out.skipped += 1;
            continue;
        }
        let data = merged_update(&u.source.file_path, &u.target.file_path)?;
        write_atomic(&u.target.file_path, &data, u.source.mtime)?;
        logging::verbose(&format!(
            "updated {} from {}",
            u.target.file_path.display(),
            u.source.file_path.display()
        ));
        out.updated += 1;
    }

    for d in &report.deletes {
        if changed_since_scan(&d.target) {
            logging::verbose(&format!(
                "skipping, changed since scan: {}",
                d.target.file_path.display()
            ));
            out.skipped += 1;
            continue;
        }
        // Mirror what Claude does on delete: marker first, then the file.
        if !d.tombstone_dest.exists() {
            write_atomic(&d.tombstone_dest, &read(&d.tombstone.file_path)?, d.tombstone.mtime)?;
        }
        fs::remove_file(&d.target.file_path)
            .with_context(|| format!("removing {}", d.target.file_path.display()))?;
        logging::verbose(&format!("deleted {}", d.target.file_path.display()));
        out.deleted += 1;
    }

    let mut done = Vec::new();
    if out.copied > 0 {
        done.push(format!("{} copied", conversations(out.copied)));
    }
    if out.updated > 0 {
        done.push(format!("{} updated", conversations(out.updated)));
    }
    if out.deleted > 0 {
        done.push(format!("{} deleted", conversations(out.deleted)));
    }
    if done.is_empty() {
        logging::info("Nothing changed (files were still being written; will retry).");
    } else {
        logging::info(&format!(
            "Done. {} — all accounts now share the same history.",
            done.join(", ")
        ));
    }
    if out.snapshot.is_some() && !at_risk.is_empty() {
        logging::info("Replaced and deleted copies were backed up. Undo with `meld restore`.");
    }
    if out.conflicts > 0 {
        logging::info(&format!(
            "{} with two versions were left untouched (see `meld status`).",
            conversations(out.conflicts)
        ));
    }
    Ok(out)
}

/// One line per planned change, grouped by kind.
pub fn print_plan(report: &DiffReport) {
    for c in &report.copies {
        logging::info(&format!("  + copy    {}", describe(&c.source)));
        logging::verbose(&format!(
            "    {} -> {}",
            c.source.file_path.display(),
            c.dest_path.display()
        ));
    }
    for u in &report.updates {
        logging::info(&format!("  ~ update  {}", describe(&u.source)));
        logging::verbose(&format!(
            "    {} -> {}",
            u.source.file_path.display(),
            u.target.file_path.display()
        ));
    }
    for d in &report.deletes {
        logging::info(&format!("  - delete  {}", describe(&d.target)));
        logging::verbose(&format!("    {}", d.target.file_path.display()));
    }
}

/// The latest copy of a conversation, with the target's account-bound
/// fields kept as they are (`scan::LOCAL_KEYS`: that org's connectors, its
/// own errors, local process handles). Falls back to the plain source bytes
/// if either file isn't a JSON object.
pub fn merged_update(source: &Path, target: &Path) -> Result<Vec<u8>> {
    use serde_json::Value;
    let src = read(source)?;
    let parse = |b: &[u8]| match serde_json::from_slice::<Value>(b) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    };
    let (Some(mut merged), Some(own)) = (parse(&src), fs::read(target).ok().as_deref().and_then(parse)) else {
        return Ok(src);
    };
    for key in crate::scan::LOCAL_KEYS {
        match own.get(*key) {
            Some(v) => merged.insert((*key).to_string(), v.clone()),
            None => merged.remove(*key),
        };
    }
    Ok(serde_json::to_vec(&Value::Object(merged))?)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    Ok(fs::read(path).with_context(|| format!("reading {}", path.display()))?)
}

/// temp-file + fsync + rename, then stamp the source mtime so newest-wins
/// stays stable across trees.
fn write_atomic(dest: &Path, data: &[u8], mtime: i64) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("meld-tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    replace_file(&tmp, dest)
        .with_context(|| format!("renaming into place {}", dest.display()))?;
    let _ = filetime_set(dest, mtime);
    Ok(())
}

/// True if the file is gone or its size/mtime moved since it was scanned.
fn changed_since_scan(e: &FileEntry) -> bool {
    let Ok(meta) = fs::metadata(&e.file_path) else {
        return true;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    meta.len() != e.size || mtime != e.mtime
}

/// "1 conversation" / "N conversations"
pub fn conversations(n: usize) -> String {
    if n == 1 {
        "1 conversation".into()
    } else {
        format!("{n} conversations")
    }
}

/// "1 conversation copy" / "N conversation copies"
fn files(n: usize) -> String {
    if n == 1 {
        "1 conversation copy".into()
    } else {
        format!("{n} conversation copies")
    }
}

/// Human-friendly one-liner for a session file: its title when known,
/// otherwise a shortened session id.
pub fn describe(e: &crate::manifest::FileEntry) -> String {
    if let Some(t) = &e.title {
        format!("\"{t}\"")
    } else if let Some(id) = &e.session_id {
        format!("untitled conversation ({})", &id[..id.len().min(8)])
    } else {
        e.relative_path.display().to_string()
    }
}

/// Preserve the source mtime on the copy; best-effort.
fn filetime_set(path: &std::path::Path, mtime: i64) -> Result<()> {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(mtime, 0))?;
    Ok(())
}
