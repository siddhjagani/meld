use meld::service::render_plist;
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
