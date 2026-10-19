use meld::{backup, config::Config, diff, discover, manifest::Manifest, restore, scan, sync};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Full sync (both opt-in policies on); tests for the copy-only default
/// turn them off explicitly.
fn test_config(root: &Path, state: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.sessions_root = root.to_path_buf();
    cfg.state_dir = state.to_path_buf();
    cfg.propagate_deletes = true;
    cfg.propagate_updates = true;
    cfg
}

#[test]
fn full_sync_is_opt_in() {
    let cfg = Config::default();
    assert!(!cfg.propagate_deletes);
    assert!(!cfg.propagate_updates);
}

fn write_session(root: &Path, account: &str, ws: &str, session: &str, body: &str) {
    let dir = root.join(account).join(ws);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("local_{session}.json"));
    fs::write(&path, body).unwrap();
    // Backdate so the "recently written" guard doesn't skip it.
    backdate(&path);
}

/// Delete a session the way Claude does: drop a `deleted_<id>` marker
/// holding the deletion time, then remove the session file.
fn delete_session(root: &Path, account: &str, ws: &str, session: &str) {
    let dir = root.join(account).join(ws);
    fs::create_dir_all(&dir).unwrap();
    let marker = dir.join(format!("deleted_{session}"));
    fs::write(&marker, "1791788350585").unwrap();
    set_mtime(&marker, 1_600_000_000);
    let _ = fs::remove_file(dir.join(format!("local_{session}.json")));
}

fn backdate(path: &Path) {
    set_mtime(path, 1_577_836_800); // 2020-01-01
}

fn set_mtime(path: &Path, secs: i64) {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn scan_all(cfg: &Config) -> Manifest {
    let roots = discover::discover_account_roots(cfg).unwrap();
    scan::scan(cfg, &roots, &Manifest::default()).unwrap()
}

fn sync_all(cfg: &Config) -> sync::SyncOutcome {
    let report = diff::compute(&scan_all(cfg), cfg.policy());
    sync::apply(cfg, &report, false, false).unwrap()
}

#[test]
fn union_mirror_copies_missing_files_both_ways() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "ws1", "y", r#"{"sessionId":"y"}"#);

    let cfg = test_config(root, state.path());
    let roots = discover::discover_account_roots(&cfg).unwrap();
    assert_eq!(roots.len(), 2);

    let m = scan_all(&cfg);
    assert_eq!(m.entries.len(), 2);

    let report = diff::compute(&m, cfg.policy());
    assert_eq!(report.copies.len(), 2, "each account is missing one file");
    assert!(report.conflicts.is_empty());

    let out = sync::apply(&cfg, &report, false, true).unwrap();
    assert_eq!(out.copied, 2);

    assert!(root.join("acct-a/ws1/local_y.json").exists());
    assert!(root.join("acct-b/ws1/local_x.json").exists());
    // Originals untouched.
    assert!(root.join("acct-a/ws1/local_x.json").exists());
    assert!(root.join("acct-b/ws1/local_y.json").exists());
}

#[test]
fn sync_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "ws1", "y", r#"{"sessionId":"y"}"#);
    delete_session(root, "acct-a", "ws1", "z");
    write_session(root, "acct-b", "ws1", "z", r#"{"sessionId":"z"}"#);

    let cfg = test_config(root, state.path());
    sync_all(&cfg);

    // Copied files inherit source mtime, so no re-backdating is needed.
    let report2 = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(report2.is_empty(), "second sync should be a no-op");
}

#[test]
fn dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "ws1", "y", r#"{"sessionId":"y"}"#);
    delete_session(root, "acct-a", "ws1", "z");
    write_session(root, "acct-b", "ws1", "z", r#"{"sessionId":"z"}"#);

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());

    sync::apply(&cfg, &report, true, true).unwrap();
    assert!(!root.join("acct-a/ws1/local_y.json").exists());
    assert!(!root.join("acct-b/ws1/local_x.json").exists());
    assert!(root.join("acct-b/ws1/local_z.json").exists());
    assert!(backup::list_snapshots(&cfg).unwrap().is_empty());
}

#[test]
fn newest_version_replaces_older_copies() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","v":2}"#);
    set_mtime(&root.join("acct-b/ws1/local_x.json"), 1_600_000_000); // newer

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.conflicts.is_empty());

    let out = sync::apply(&cfg, &report, false, false).unwrap();
    assert_eq!(out.updated, 1);
    let a = fs::read_to_string(root.join("acct-a/ws1/local_x.json")).unwrap();
    assert!(a.contains(r#""v":2"#), "older copy replaced by newest");

    // The replaced copy is in the backup.
    let snap = out.snapshot.expect("backup taken before overwrite");
    let saved = fs::read_to_string(snap.join("acct-a/ws1/local_x.json")).unwrap();
    assert!(saved.contains(r#""v":1"#));
}

#[test]
fn volatile_field_drift_is_not_an_update() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","lastFocusedAt":1}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","lastFocusedAt":2}"#);
    set_mtime(&root.join("acct-b/ws1/local_x.json"), 1_600_000_000);

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(report.is_empty());
    assert!(report.conflicts.is_empty());
}

#[test]
fn same_mtime_different_content_is_a_conflict() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","v":2}"#);

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.conflicts.len(), 1);
    assert!(report.updates.is_empty(), "no strictly-newest copy; keep both");
}

#[test]
fn existing_files_are_never_overwritten_when_updates_are_off() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","v":2}"#);
    set_mtime(&root.join("acct-b/ws1/local_x.json"), 1_600_000_000);

    let mut cfg = test_config(root, state.path());
    cfg.propagate_updates = false;
    let report = diff::compute(&scan_all(&cfg), cfg.policy());

    assert_eq!(report.conflicts.len(), 1);
    assert!(report.is_empty(), "both trees have the path; no change");

    sync::apply(&cfg, &report, false, true).unwrap();
    let a = fs::read_to_string(root.join("acct-a/ws1/local_x.json")).unwrap();
    let b = fs::read_to_string(root.join("acct-b/ws1/local_x.json")).unwrap();
    assert!(a.contains(r#""v":1"#));
    assert!(b.contains(r#""v":2"#));
}

#[test]
fn deletion_in_one_account_deletes_everywhere() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "ws1", "x");
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-c", "ws1", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    let out = sync_all(&cfg);
    assert_eq!(out.deleted, 2);
    assert_eq!(out.copied, 0, "a deleted conversation is never copied back");

    for acct in ["acct-a", "acct-b", "acct-c"] {
        assert!(!root.join(acct).join("ws1/local_x.json").exists());
        assert!(root.join(acct).join("ws1/deleted_x").exists(), "{acct} has the marker");
    }
    // Deleted copies are in the backup.
    let snap = out.snapshot.unwrap();
    assert!(snap.join("acct-b/ws1/local_x.json").exists());
    assert!(snap.join("acct-c/ws1/local_x.json").exists());
}

#[test]
fn deletes_are_kept_local_when_turned_off() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "ws1", "x");
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x"}"#);

    let mut cfg = test_config(root, state.path());
    cfg.propagate_deletes = false;
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(report.deletes.is_empty());
    sync::apply(&cfg, &report, false, true).unwrap();
    assert!(root.join("acct-b/ws1/local_x.json").exists());
}

#[test]
fn non_session_files_are_not_synced() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x"}"#);
    let tasks = root.join("acct-a/ws1/scheduled-tasks.json");
    fs::write(&tasks, "[]").unwrap();
    backdate(&tasks);
    fs::create_dir_all(root.join("acct-b")).unwrap();

    let cfg = test_config(root, state.path());
    sync_all(&cfg);
    assert!(root.join("acct-b/ws1/local_x.json").exists());
    assert!(!root.join("acct-b/ws1/scheduled-tasks.json").exists());
}

#[test]
fn restore_undoes_a_propagated_deletion() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "ws1", "x");
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    sync_all(&cfg);
    assert!(!root.join("acct-b/ws1/local_x.json").exists());

    restore::restore(&cfg, None, false).unwrap();
    assert!(root.join("acct-b/ws1/local_x.json").exists());
    assert!(!root.join("acct-a/ws1/deleted_x").exists(), "un-deleted everywhere");
    assert!(!root.join("acct-b/ws1/deleted_x").exists());
    // The restore itself was backed up first, so it can be undone too.
    assert_eq!(backup::list_snapshots(&cfg).unwrap().len(), 2);
}

#[test]
fn new_account_folder_is_filled_on_next_sync() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    // A brand-new (empty) account folder appears later.
    fs::create_dir_all(root.join("acct-new")).unwrap();

    let roots = discover::discover_account_roots(&cfg).unwrap();
    assert_eq!(roots.len(), 2);

    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.copies.len(), 1);
    sync::apply(&cfg, &report, false, true).unwrap();
    assert!(root.join("acct-new/ws1/local_x.json").exists());
}

// ---- "newest" is decided by real edits, not by file modification time ----

fn scan_after(cfg: &Config, previous: &Manifest) -> Manifest {
    let roots = discover::discover_account_roots(cfg).unwrap();
    scan::scan(cfg, &roots, previous).unwrap()
}

/// Two accounts holding the same conversation `x`; returns the scan that
/// records their agreed version.
fn agreed_pair(cfg: &Config, root: &Path) -> Manifest {
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","title":"v1"}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","title":"v1"}"#);
    let m = scan_all(cfg);
    assert_eq!(m.base.len(), 1, "agreed version is recorded");
    m
}

#[test]
fn opening_a_stale_copy_does_not_beat_a_real_edit() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let cfg = test_config(root, state.path());
    let m1 = agreed_pair(&cfg, root);

    // A is really edited; B is only opened (Claude bumps lastFocusedAt and
    // the mtime) so its file looks newer but says the same thing as before.
    let a = root.join("acct-a/ws1/local_x.json");
    let b = root.join("acct-b/ws1/local_x.json");
    fs::write(&a, r#"{"sessionId":"x","title":"v2 edited in A"}"#).unwrap();
    set_mtime(&a, 1_650_000_000);
    fs::write(&b, r#"{"sessionId":"x","title":"v1","lastFocusedAt":99}"#).unwrap();
    set_mtime(&b, 1_700_000_000);

    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.updates[0].target.account_root.ends_with("acct-b"));
    assert!(report.conflicts.is_empty());

    sync::apply(&cfg, &report, false, false).unwrap();
    assert!(fs::read_to_string(&a).unwrap().contains("v2 edited in A"), "A keeps its edit");
    assert!(fs::read_to_string(&b).unwrap().contains("v2 edited in A"), "B receives it");
}

#[test]
fn edited_in_both_accounts_is_a_conflict_even_if_one_is_newer() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let cfg = test_config(root, state.path());
    let m1 = agreed_pair(&cfg, root);

    let a = root.join("acct-a/ws1/local_x.json");
    let b = root.join("acct-b/ws1/local_x.json");
    fs::write(&a, r#"{"sessionId":"x","title":"edit in A"}"#).unwrap();
    set_mtime(&a, 1_650_000_000);
    fs::write(&b, r#"{"sessionId":"x","title":"edit in B"}"#).unwrap();
    set_mtime(&b, 1_700_000_000);

    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.conflicts.len(), 1);
    assert!(report.updates.is_empty(), "two real edits: never pick one by mtime");
}

#[test]
fn deleting_a_chat_that_was_edited_elsewhere_keeps_the_edit() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let cfg = test_config(root, state.path());
    let m1 = agreed_pair(&cfg, root);

    // Deleted in A; meanwhile B was continued (a real edit meld had not
    // yet synced).
    delete_session(root, "acct-a", "ws1", "x");
    let b = root.join("acct-b/ws1/local_x.json");
    fs::write(&b, r#"{"sessionId":"x","title":"continued in B"}"#).unwrap();
    backdate(&b);

    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert!(report.deletes.is_empty());
    assert_eq!(report.conflicts.len(), 1, "surfaced to the user");
    sync::apply(&cfg, &report, false, false).unwrap();
    assert!(fs::read_to_string(&b).unwrap().contains("continued in B"));
}

#[test]
fn deleting_a_chat_nobody_edited_still_propagates() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let cfg = test_config(root, state.path());
    let m1 = agreed_pair(&cfg, root);

    delete_session(root, "acct-a", "ws1", "x");
    // B was merely opened: volatile fields and mtime move, content doesn't.
    let b = root.join("acct-b/ws1/local_x.json");
    fs::write(&b, r#"{"sessionId":"x","title":"v1","lastFocusedAt":99}"#).unwrap();
    set_mtime(&b, 1_700_000_000);

    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.deletes.len(), 1);
    assert!(report.conflicts.is_empty());
    sync::apply(&cfg, &report, false, false).unwrap();
    assert!(!b.exists());
}

#[test]
fn a_file_still_being_written_does_not_record_an_agreed_version() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    // A's copy is brand new (inside the quiet window, so the scan skips it);
    // B's older copy must not be recorded as "agreed" on its own.
    write_session(root, "acct-a", "ws1", "x", r#"{"sessionId":"x","title":"fresh"}"#);
    write_session(root, "acct-b", "ws1", "x", r#"{"sessionId":"x","title":"old"}"#);
    backdate(&root.join("acct-b/ws1/local_x.json"));

    let cfg = test_config(root, state.path());
    assert!(scan_all(&cfg).base.is_empty());
}

#[test]
fn manifest_from_before_this_change_still_loads() {
    let m: Manifest = serde_json::from_str(
        r#"{"generated_at":"","sessions_root":"/","account_roots":[],"entries":[]}"#,
    )
    .unwrap();
    assert!(m.base.is_empty() && m.tombstones.is_empty());
}
