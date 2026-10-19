use meld::{backup, config::Config, discover, restore};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn test_config(root: &Path, state: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.sessions_root = root.to_path_buf();
    cfg.state_dir = state.to_path_buf();
    cfg
}

#[test]
fn snapshot_and_restore_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let file = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"sessionId":"x","v":"original"}"#).unwrap();

    let cfg = test_config(root, state.path());
    let roots = discover::discover_account_roots(&cfg).unwrap();

    backup::snapshot(&cfg, &roots).unwrap();
    assert_eq!(backup::list_snapshots(&cfg).unwrap().len(), 1);

    // Corrupt the file, then restore.
    fs::write(&file, "garbage").unwrap();
    let restored = restore::restore(&cfg, None, false).unwrap();
    assert_eq!(restored, 1);
    let body = fs::read_to_string(&file).unwrap();
    assert!(body.contains("original"));
}

#[test]
fn restore_does_not_delete_new_files() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let old = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, r#"{"sessionId":"x"}"#).unwrap();

    let cfg = test_config(root, state.path());
    let roots = discover::discover_account_roots(&cfg).unwrap();
    backup::snapshot(&cfg, &roots).unwrap();

    // A new file appears after the snapshot.
    let new = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_new.json");
    fs::write(&new, r#"{"sessionId":"new"}"#).unwrap();

    restore::restore(&cfg, None, false).unwrap();
    assert!(new.exists(), "restore must never delete files");
}

#[test]
fn full_snapshots_have_their_own_smaller_cap() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let file = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"sessionId":"x"}"#).unwrap();

    let mut cfg = test_config(root, state.path());
    cfg.max_full_snapshots = 2;
    let roots = discover::discover_account_roots(&cfg).unwrap();
    let pause = || std::thread::sleep(std::time::Duration::from_millis(5));

    for _ in 0..3 {
        backup::snapshot_files(&cfg, &[file.clone()], None).unwrap();
        pause();
    }
    for _ in 0..4 {
        backup::snapshot(&cfg, &roots).unwrap();
        pause();
    }

    let names: Vec<String> = backup::list_snapshots(&cfg)
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let full = names.iter().filter(|n| n.ends_with("-full")).count();
    assert_eq!(full, 2, "only the newest full snapshots are kept");
    assert_eq!(names.len() - full, 3, "small per-file snapshots are untouched");
}

#[test]
fn total_snapshot_cap_still_applies() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let file = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"sessionId":"x"}"#).unwrap();

    let mut cfg = test_config(root, state.path());
    cfg.max_snapshots = 3;
    for _ in 0..6 {
        backup::snapshot_files(&cfg, &[file.clone()], None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(backup::list_snapshots(&cfg).unwrap().len(), 3);
}

#[test]
fn restore_can_use_a_full_snapshot_by_name() {
    let tmp = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let root = tmp.path();
    let file = root.join("acct-a/11111111-1111-4111-8111-111111111111/local_x.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"sessionId":"x","v":"original"}"#).unwrap();

    let cfg = test_config(root, state.path());
    let roots = discover::discover_account_roots(&cfg).unwrap();
    let snap = backup::snapshot(&cfg, &roots).unwrap();
    let name = snap.file_name().unwrap().to_str().unwrap().to_string();

    fs::write(&file, "garbage").unwrap();
    restore::restore(&cfg, Some(&name), false).unwrap();
    assert!(fs::read_to_string(&file).unwrap().contains("original"));
}
