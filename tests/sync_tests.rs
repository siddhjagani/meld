use meld::{backup, config::{Config, DeletedChats}, diff, discover, manifest::Manifest, restore, scan, sync};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Full sync (both policies on); tests for the other modes turn them off
/// explicitly.
fn test_config(root: &Path, state: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.sessions_root = root.to_path_buf();
    cfg.state_dir = state.to_path_buf();
    cfg.deleted_chats = DeletedChats::DeleteEverywhere;
    cfg.propagate_updates = true;
    cfg
}

#[test]
fn updates_are_on_by_default_and_deletes_are_opt_in() {
    let cfg = Config::default();
    assert!(cfg.propagate_updates);
    assert_eq!(cfg.policy().deletes, DeletedChats::ComeBack);
}

#[test]
fn config_file_without_the_keys_gets_the_defaults_and_explicit_values_win() {
    let old: Config = toml::from_str("max_snapshots = 10").unwrap();
    assert!(old.propagate_updates && old.policy().deletes == DeletedChats::ComeBack);
    let off: Config = toml::from_str("propagate_updates = false").unwrap();
    assert!(!off.propagate_updates);
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
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "y", r#"{"sessionId":"y"}"#);

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

    assert!(root.join("acct-a/11111111-1111-4111-8111-111111111111/local_y.json").exists());
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    // Originals untouched.
    assert!(root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_y.json").exists());
}

#[test]
fn sync_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "y", r#"{"sessionId":"y"}"#);
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "z");
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "z", r#"{"sessionId":"z"}"#);

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
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "y", r#"{"sessionId":"y"}"#);
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "z");
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "z", r#"{"sessionId":"z"}"#);

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());

    sync::apply(&cfg, &report, true, true).unwrap();
    assert!(!root.join("acct-a/11111111-1111-4111-8111-111111111111/local_y.json").exists());
    assert!(!root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_z.json").exists());
    assert!(backup::list_snapshots(&cfg).unwrap().is_empty());
}

#[test]
fn newest_version_replaces_older_copies() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":2}"#);
    set_mtime(&root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json"), 1_600_000_000); // newer

    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.conflicts.is_empty());

    let out = sync::apply(&cfg, &report, false, false).unwrap();
    assert_eq!(out.updated, 1);
    let a = fs::read_to_string(root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json")).unwrap();
    assert!(a.contains(r#""v":2"#), "older copy replaced by newest");

    // The replaced copy is in the backup.
    let snap = out.snapshot.expect("backup taken before overwrite");
    let saved = fs::read_to_string(snap.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json")).unwrap();
    assert!(saved.contains(r#""v":1"#));
}

#[test]
fn volatile_field_drift_is_not_an_update() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","lastFocusedAt":1}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","lastFocusedAt":2}"#);
    set_mtime(&root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json"), 1_600_000_000);

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
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":2}"#);

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
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":1}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","v":2}"#);
    set_mtime(&root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json"), 1_600_000_000);

    let mut cfg = test_config(root, state.path());
    cfg.propagate_updates = false;
    let report = diff::compute(&scan_all(&cfg), cfg.policy());

    assert_eq!(report.conflicts.len(), 1);
    assert!(report.is_empty(), "both trees have the path; no change");

    sync::apply(&cfg, &report, false, true).unwrap();
    let a = fs::read_to_string(root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json")).unwrap();
    let b = fs::read_to_string(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json")).unwrap();
    assert!(a.contains(r#""v":1"#));
    assert!(b.contains(r#""v":2"#));
}

#[test]
fn deletion_in_one_account_deletes_everywhere() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x");
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);
    write_session(root, "acct-c", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    let out = sync_all(&cfg);
    assert_eq!(out.deleted, 2);
    assert_eq!(out.copied, 0, "a deleted conversation is never copied back");

    for acct in ["acct-a", "acct-b", "acct-c"] {
        assert!(!root.join(acct).join("11111111-1111-4111-8111-111111111111/local_x.json").exists());
        assert!(root.join(acct).join("11111111-1111-4111-8111-111111111111/deleted_x").exists(), "{acct} has the marker");
    }
    // Deleted copies are in the backup.
    let snap = out.snapshot.unwrap();
    assert!(snap.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    assert!(snap.join("acct-c/11111111-1111-4111-8111-111111111111/local_x.json").exists());
}

#[test]
fn deletes_are_kept_local_when_turned_off() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x");
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);

    let mut cfg = test_config(root, state.path());
    cfg.deleted_chats = DeletedChats::ComeBack;
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(report.deletes.is_empty());
    sync::apply(&cfg, &report, false, true).unwrap();
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
}

#[test]
fn non_session_files_are_not_synced() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);
    let tasks = root.join("acct-a/11111111-1111-4111-8111-111111111111/scheduled-tasks.json");
    fs::write(&tasks, "[]").unwrap();
    backdate(&tasks);
    fs::create_dir_all(root.join("acct-b")).unwrap();

    let cfg = test_config(root, state.path());
    sync_all(&cfg);
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    assert!(!root.join("acct-b/11111111-1111-4111-8111-111111111111/scheduled-tasks.json").exists());
}

#[test]
fn restore_undoes_a_propagated_deletion() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x");
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    sync_all(&cfg);
    assert!(!root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());

    restore::restore(&cfg, None, false).unwrap();
    assert!(root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json").exists());
    assert!(!root.join("acct-a/11111111-1111-4111-8111-111111111111/deleted_x").exists(), "un-deleted everywhere");
    assert!(!root.join("acct-b/11111111-1111-4111-8111-111111111111/deleted_x").exists());
    // The restore itself was backed up first, so it can be undone too.
    assert_eq!(backup::list_snapshots(&cfg).unwrap().len(), 2);
}

#[test]
fn new_account_folder_is_filled_on_next_sync() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x"}"#);

    let cfg = test_config(root, state.path());
    // A brand-new (empty) account folder appears later.
    fs::create_dir_all(root.join("acct-new")).unwrap();

    let roots = discover::discover_account_roots(&cfg).unwrap();
    assert_eq!(roots.len(), 2);

    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.copies.len(), 1);
    sync::apply(&cfg, &report, false, true).unwrap();
    assert!(root.join("acct-new/11111111-1111-4111-8111-111111111111/local_x.json").exists());
}

// ---- "newest" is decided by real edits, not by file modification time ----

fn scan_after(cfg: &Config, previous: &Manifest) -> Manifest {
    let roots = discover::discover_account_roots(cfg).unwrap();
    scan::scan(cfg, &roots, previous).unwrap()
}

/// Two accounts holding the same conversation `x`; returns the scan that
/// records their agreed version.
fn agreed_pair(cfg: &Config, root: &Path) -> Manifest {
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","title":"v1"}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","title":"v1"}"#);
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
    let a = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    let b = root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json");
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

    let a = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    let b = root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json");
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
    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x");
    let b = root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json");
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

    delete_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x");
    // B was merely opened: volatile fields and mtime move, content doesn't.
    let b = root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json");
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
    write_session(root, "acct-a", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","title":"fresh"}"#);
    write_session(root, "acct-b", "11111111-1111-4111-8111-111111111111", "x", r#"{"sessionId":"x","title":"old"}"#);
    backdate(&root.join("acct-b/11111111-1111-4111-8111-111111111111/local_x.json"));

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

// ---- every conversation in every organization of every account ----

const ORG_PERSONAL: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const ORG_TEAM: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const ORG_MAX: &str = "cccccccc-0000-4000-8000-000000000003";

fn chat_at(root: &Path, account: &str, org: &str, chat: &str) -> std::path::PathBuf {
    root.join(account).join(org).join(format!("local_{chat}.json"))
}

#[test]
fn chats_reach_every_organization_of_every_account() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    // Account 1: personal + team. Account 2: max (+ an empty personal org).
    write_session(root, "acct-1", ORG_PERSONAL, "p", r#"{"sessionId":"p"}"#);
    write_session(root, "acct-1", ORG_TEAM, "t", r#"{"sessionId":"t"}"#);
    write_session(root, "acct-2", ORG_MAX, "m", r#"{"sessionId":"m"}"#);
    fs::create_dir_all(root.join("acct-2").join(ORG_PERSONAL)).unwrap();

    let cfg = test_config(root, state.path());
    let out = sync_all(&cfg);
    assert_eq!(out.copied, 4 * 3 - 3, "4 org folders x 3 chats, minus the 3 originals");
    for (acct, org) in [("acct-1", ORG_PERSONAL), ("acct-1", ORG_TEAM), ("acct-2", ORG_MAX), ("acct-2", ORG_PERSONAL)] {
        for chat in ["p", "t", "m"] {
            assert!(chat_at(root, acct, org, chat).exists(), "{acct}/{org} is missing {chat}");
        }
    }
    let again = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(again.is_empty(), "second sync is a no-op");
}

#[test]
fn folders_that_are_not_claude_organizations_are_left_alone() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-1", ORG_PERSONAL, "p", r#"{"sessionId":"p"}"#);
    write_session(root, "acct-1", "notes-5a-sidh", "junk", r#"{"sessionId":"junk"}"#);
    write_session(root, "acct-2", ORG_MAX, "m", r#"{"sessionId":"m"}"#);
    // Claude's own sub-folders inside an organization are not conversations.
    let nested = root.join("acct-2").join(ORG_MAX).join("backlog");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("local_q.json"), "{}").unwrap();
    backdate(&nested.join("local_q.json"));

    let cfg = test_config(root, state.path());
    sync_all(&cfg);
    assert!(!root.join("acct-1/notes-5a-sidh/local_p.json").exists(), "not a target");
    assert!(!chat_at(root, "acct-1", ORG_PERSONAL, "junk").exists(), "not a source");
    assert!(!chat_at(root, "acct-1", ORG_PERSONAL, "q").exists());
    assert!(chat_at(root, "acct-1", ORG_PERSONAL, "m").exists());
}

#[test]
fn a_continued_chat_updates_every_organization() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-1", ORG_TEAM, "x", r#"{"sessionId":"x","title":"v1"}"#);
    write_session(root, "acct-2", ORG_MAX, "x", r#"{"sessionId":"x","title":"v1"}"#);
    let cfg = test_config(root, state.path());
    let m1 = scan_all(&cfg);

    let team = chat_at(root, "acct-1", ORG_TEAM, "x");
    fs::write(&team, r#"{"sessionId":"x","title":"v2 continued on team"}"#).unwrap();
    set_mtime(&team, 1_650_000_000);
    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    sync::apply(&cfg, &report, false, false).unwrap();
    let max = fs::read_to_string(chat_at(root, "acct-2", ORG_MAX, "x")).unwrap();
    assert!(max.contains("v2 continued on team"));
}

// ---- Claude writing an old in-memory copy back ----

#[test]
fn an_old_copy_written_back_is_replaced_not_spread() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x","cliSessionId":"old"}"#);
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x","cliSessionId":"old"}"#);
    let cfg = test_config(root, state.path());
    let a = chat_at(root, "acct-a", ORG_TEAM, "x");
    let b = chat_at(root, "acct-b", ORG_MAX, "x");

    // Continued in A; meld brings B up to date.
    let m1 = scan_all(&cfg);
    fs::write(&a, r#"{"sessionId":"x","cliSessionId":"new"}"#).unwrap();
    set_mtime(&a, 1_650_000_000);
    let m2 = scan_after(&cfg, &m1);
    sync::apply(&cfg, &diff::compute(&m2, cfg.policy()), false, false).unwrap();
    let m3 = scan_after(&cfg, &m2);
    assert!(fs::read_to_string(&b).unwrap().contains("new"));

    // Claude, still running in B with the old list in memory, saves it back.
    fs::write(&b, r#"{"sessionId":"x","cliSessionId":"old","lastFocusedAt":5}"#).unwrap();
    set_mtime(&b, 1_700_000_000);
    let report = diff::compute(&scan_after(&cfg, &m3), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.updates[0].target.account_root.ends_with("acct-b"), "B is repaired, A untouched");
    assert!(report.conflicts.is_empty());
    sync::apply(&cfg, &report, false, false).unwrap();
    assert!(fs::read_to_string(&a).unwrap().contains("new"));
    assert!(fs::read_to_string(&b).unwrap().contains("new"));
}

// ---- deleted chats ----

fn deleted_setup(root: &Path) {
    // Deleted in A's team org; B and C still have it.
    write_session(root, "acct-a", ORG_TEAM, "keep", r#"{"sessionId":"keep"}"#);
    delete_session(root, "acct-a", ORG_TEAM, "x");
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x"}"#);
    fs::create_dir_all(root.join("acct-c").join(ORG_PERSONAL)).unwrap();
}

#[test]
fn by_default_a_deleted_chat_comes_back() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    deleted_setup(root);
    let mut cfg = test_config(root, state.path());
    cfg.deleted_chats = DeletedChats::default();
    sync_all(&cfg);
    assert!(chat_at(root, "acct-a", ORG_TEAM, "x").exists(), "came back");
    assert!(chat_at(root, "acct-b", ORG_MAX, "x").exists());
    assert!(chat_at(root, "acct-c", ORG_PERSONAL, "x").exists());
}

#[test]
fn stay_deleted_here_keeps_it_out_of_that_org_only() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    deleted_setup(root);
    let mut cfg = test_config(root, state.path());
    cfg.deleted_chats = DeletedChats::StayDeletedHere;
    sync_all(&cfg);
    assert!(!chat_at(root, "acct-a", ORG_TEAM, "x").exists(), "stays deleted where you deleted it");
    assert!(chat_at(root, "acct-b", ORG_MAX, "x").exists(), "never deleted elsewhere");
    assert!(chat_at(root, "acct-c", ORG_PERSONAL, "x").exists(), "other orgs still get it");
    let again = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(again.is_empty());
}

#[test]
fn delete_everywhere_reaches_other_accounts_and_orgs() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    deleted_setup(root);
    let mut cfg = test_config(root, state.path());
    cfg.deleted_chats = DeletedChats::DeleteEverywhere;
    let out = sync_all(&cfg);
    assert_eq!(out.deleted, 1);
    assert!(!chat_at(root, "acct-b", ORG_MAX, "x").exists());
    assert!(root.join("acct-b").join(ORG_MAX).join("deleted_x").exists(), "marker written like Claude does");
    assert!(!chat_at(root, "acct-c", ORG_PERSONAL, "x").exists(), "not copied anywhere");
}

#[test]
fn old_propagate_deletes_setting_still_means_everywhere() {
    let cfg: Config = toml::from_str("propagate_deletes = true").unwrap();
    assert_eq!(cfg.policy().deletes, DeletedChats::DeleteEverywhere);
    let cfg: Config = toml::from_str(r#"deleted_chats = "stay_deleted_here""#).unwrap();
    assert_eq!(cfg.policy().deletes, DeletedChats::StayDeletedHere);
}

// ---- account-bound fields stay with their account ----

#[test]
fn an_update_keeps_the_targets_own_connectors_and_errors() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let team = chat_at(root, "acct-a", ORG_TEAM, "x");
    let max = chat_at(root, "acct-b", ORG_MAX, "x");
    write_session(root, "acct-a", ORG_TEAM, "x",
        r#"{"sessionId":"x","cliSessionId":"c1","error":"usage limit reached","remoteMcpServersConfig":["team-only"]}"#);
    write_session(root, "acct-b", ORG_MAX, "x",
        r#"{"sessionId":"x","cliSessionId":"c1","remoteMcpServersConfig":["max-only"]}"#);
    let cfg = test_config(root, state.path());
    let m1 = scan_all(&cfg);
    assert!(diff::compute(&m1, cfg.policy()).is_empty(), "account-bound fields are not a difference");

    // Continued on team (new cliSessionId) until the limit hit.
    fs::write(&team, r#"{"sessionId":"x","cliSessionId":"c2","error":"usage limit reached","remoteMcpServersConfig":["team-only"]}"#).unwrap();
    set_mtime(&team, 1_650_000_000);
    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    sync::apply(&cfg, &report, false, false).unwrap();

    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&max).unwrap()).unwrap();
    assert_eq!(v["cliSessionId"], "c2", "the conversation moved on");
    assert_eq!(v["remoteMcpServersConfig"][0], "max-only", "Max keeps its connectors");
    assert!(v.get("error").is_none(), "the team's limit error doesn't follow you to Max");
    let again = diff::compute(&scan_all(&cfg), cfg.policy());
    assert!(again.updates.is_empty() && again.conflicts.is_empty(), "stable after the merge");
}

#[test]
fn manifest_from_an_older_hash_version_is_not_trusted() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x"}"#);
    let cfg = test_config(root, state.path());
    let mut old = scan_all(&cfg);
    old.hash_version = 1;
    for e in &mut old.entries {
        e.content_hash = "stale-formula".into();
    }
    old.base.insert("local_x.json".into(), "stale-formula".into());
    let fresh = scan_after(&cfg, &old);
    assert_ne!(fresh.entries[0].content_hash, "stale-formula");
    assert_ne!(fresh.base.get(std::path::Path::new("local_x.json")).map(String::as_str), Some("stale-formula"));
}

#[test]
fn first_sync_picks_the_copy_that_continued_the_transcript() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    // A continued the chat (its transcript c2 follows c1), but B's file was
    // touched later. No history yet: lineage must win over file time.
    write_session(root, "acct-a", ORG_TEAM, "x",
        r#"{"sessionId":"x","cliSessionId":"c2","priorCliSessionIds":["c1"],"lastActivityAt":1}"#);
    write_session(root, "acct-b", ORG_MAX, "x",
        r#"{"sessionId":"x","cliSessionId":"c1","lastActivityAt":999}"#);
    set_mtime(&chat_at(root, "acct-b", ORG_MAX, "x"), 1_700_000_000);
    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.updates[0].target.account_root.ends_with("acct-b"));
}

#[test]
fn first_sync_prefers_latest_activity_over_file_time() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x","title":"active","lastActivityAt":2000}"#);
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x","title":"touched","lastActivityAt":1000}"#);
    set_mtime(&chat_at(root, "acct-b", ORG_MAX, "x"), 1_700_000_000); // opened later
    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.updates.len(), 1);
    assert!(report.updates[0].target.account_root.ends_with("acct-b"));
}

// ---- two Claude windows saving the same conversation ----

#[test]
fn same_transcript_edited_in_two_accounts_takes_the_most_recent() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x","cliSessionId":"c1","lastAssistantUuid":"u0","lastActivityAt":1}"#);
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x","cliSessionId":"c1","lastAssistantUuid":"u0","lastActivityAt":1}"#);
    let cfg = test_config(root, state.path());
    let m1 = scan_all(&cfg);

    let a = chat_at(root, "acct-a", ORG_TEAM, "x");
    let b = chat_at(root, "acct-b", ORG_MAX, "x");
    fs::write(&a, r#"{"sessionId":"x","cliSessionId":"c1","lastAssistantUuid":"u1","lastActivityAt":10}"#).unwrap();
    fs::write(&b, r#"{"sessionId":"x","cliSessionId":"c1","lastAssistantUuid":"u2","lastActivityAt":20}"#).unwrap();
    backdate(&a);
    backdate(&b);
    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert!(report.conflicts.is_empty(), "one transcript: not a conflict");
    assert_eq!(report.updates.len(), 1);
    assert!(report.updates[0].target.account_root.ends_with("acct-a"), "b is the most recent");
}

#[test]
fn different_transcripts_edited_in_two_accounts_stay_a_conflict() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x","cliSessionId":"c1"}"#);
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x","cliSessionId":"c1"}"#);
    let cfg = test_config(root, state.path());
    let m1 = scan_all(&cfg);
    let a = chat_at(root, "acct-a", ORG_TEAM, "x");
    let b = chat_at(root, "acct-b", ORG_MAX, "x");
    fs::write(&a, r#"{"sessionId":"x","cliSessionId":"fork-a","priorCliSessionIds":["c1"]}"#).unwrap();
    fs::write(&b, r#"{"sessionId":"x","cliSessionId":"fork-b","priorCliSessionIds":["c1"]}"#).unwrap();
    backdate(&a);
    backdate(&b);
    let report = diff::compute(&scan_after(&cfg, &m1), cfg.policy());
    assert_eq!(report.conflicts.len(), 1, "two separate continuations: keep both");
    assert!(report.updates.is_empty());
}

#[test]
fn a_plan_made_moot_by_writes_takes_no_backup() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    write_session(root, "acct-a", ORG_TEAM, "x", r#"{"sessionId":"x"}"#);
    fs::create_dir_all(root.join("acct-b").join(ORG_MAX)).unwrap();
    let cfg = test_config(root, state.path());
    let report = diff::compute(&scan_all(&cfg), cfg.policy());
    assert_eq!(report.copies.len(), 1);
    // Claude writes the file itself before meld gets to it.
    write_session(root, "acct-b", ORG_MAX, "x", r#"{"sessionId":"x"}"#);
    let out = sync::apply(&cfg, &report, false, false).unwrap();
    assert!(out.snapshot.is_none(), "no 2000-file backup for nothing");
    assert!(backup::list_snapshots(&cfg).unwrap().is_empty());
}
