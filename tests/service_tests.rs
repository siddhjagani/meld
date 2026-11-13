use meld::service::{render_login_command, render_plist};
use std::path::Path;

#[test]
fn plist_runs_watch_with_the_given_path_and_escapes_xml() {
    let plist = render_plist(
        Path::new("/opt/homebrew/bin/meld & co/meld"),
        Path::new("/Users/me/Library/Logs/meld.log"),
    );
    assert!(plist.contains("<string>/opt/homebrew/bin/meld &amp; co/meld</string>"));
    assert!(plist.contains("<string>watch</string>"));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert!(plist.contains("<string>/Users/me/Library/Logs/meld.log</string>"));
}

#[test]
fn windows_login_command_quotes_paths_with_spaces() {
    let line = render_login_command(
        Path::new(r"C:\Windows\System32\conhost.exe"),
        Path::new(r"C:\Users\Tanika Tech\AppData\Local\meld\bin\meld.exe"),
    );
    assert_eq!(
        line,
        r#""C:\Windows\System32\conhost.exe" --headless "C:\Users\Tanika Tech\AppData\Local\meld\bin\meld.exe" service run"#
    );
}

#[test]
fn missing_session_folder_error_lists_where_meld_searched() {
    let mut cfg = meld::config::Config::default();
    cfg.sessions_root = std::env::temp_dir().join("meld-test-no-such-sessions-root");
    let err = meld::discover::discover_account_roots(&cfg).unwrap_err().to_string();
    assert!(err.contains("searched every Claude folder under:"), "{err}");
    for loc in meld::config::search_locations() {
        assert!(err.contains(&loc.display().to_string()), "{err}");
    }
}
