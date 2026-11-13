use crate::errors::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Meld configuration. Lives at ~/.meld/config.toml; every field has a
/// sensible default so the tool works with no config file at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Parent directory that contains one folder per account.
    pub sessions_root: PathBuf,
    /// Where meld keeps its manifest, backups and lock file.
    pub state_dir: PathBuf,
    /// File-watcher debounce in milliseconds.
    pub debounce_ms: u64,
    /// Keep at most this many snapshots (oldest pruned first).
    pub max_snapshots: usize,
    /// Full snapshots copy every account tree (tens of MB each), so they get
    /// their own, smaller cap on top of `max_snapshots`.
    pub max_full_snapshots: usize,
    /// File name patterns to ignore (exact basename match).
    pub ignore: Vec<String>,
    /// What happens to a conversation you delete in one account or
    /// organization. Default: it comes back from the others.
    pub deleted_chats: DeletedChats,
    /// Older setting: `true` means `deleted_chats = "delete_everywhere"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub propagate_deletes: Option<bool>,
    /// On by default: when accounts hold different versions of a conversation,
    /// the copy that was edited replaces the others (the replaced copies are
    /// backed up). Set to false to keep both versions as a conflict.
    pub propagate_updates: bool,
}

/// What a sync does with a conversation that was deleted somewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletedChats {
    /// Copied back from the accounts that still have it (nothing is deleted).
    #[default]
    ComeBack,
    /// Stays deleted in the organization you deleted it in; every other
    /// account and organization keeps it.
    StayDeletedHere,
    /// Deleted in every account and organization (copies are backed up).
    DeleteEverywhere,
}

/// The parts of the config that decide what a sync does.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub deletes: DeletedChats,
    pub propagate_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sessions_root: default_sessions_root(),
            state_dir: home_dir().join(".meld"),
            debounce_ms: 1500,
            max_snapshots: 200,
            max_full_snapshots: 10,
            ignore: vec![".DS_Store".into(), "desktop.ini".into(), "Thumbs.db".into()],
            deleted_chats: DeletedChats::ComeBack,
            propagate_deletes: None,
            propagate_updates: true,
        }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// True for folder names that can belong to Claude: `Claude`, `Claude-3p`,
/// a beta channel, the MSIX package `Claude_<hash>`, `AnthropicClaude`, ...
fn claude_named(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| {
        let n = n.to_ascii_lowercase();
        n.contains("claude") || n.contains("anthropic")
    })
}

/// Levels the fallback search adds when the known layout holds no store.
const DEEP_EXTRA: usize = 4;

/// Every `claude-code-sessions` folder inside a Claude-named child of
/// `base`, at most `depth` levels below `base`. Searched instead of assumed,
/// so a different package hash, a `LocalState` folder or another channel is
/// still found; bounded and limited to Claude-named folders so a large
/// AppData stays fast. Sorted, so the order is stable.
fn find_stores(base: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut walk = walkdir::WalkDir::new(base)
        .min_depth(1)
        .max_depth(depth)
        .follow_links(true) // a junctioned AppData folder still counts
        .sort_by_file_name()
        .into_iter();
    while let Some(entry) = walk.next() {
        let Ok(e) = entry else { continue };
        if !e.file_type().is_dir() {
            continue;
        }
        if e.depth() == 1 && !claude_named(e.file_name()) {
            walk.skip_current_dir();
        } else if e.file_name().eq_ignore_ascii_case("claude-code-sessions") {
            found.push(e.into_path());
            walk.skip_current_dir();
        }
    }
    found
}

/// Windows search bases, given `%LOCALAPPDATA%` and `%APPDATA%`, each with
/// how deep a store sits in the known layout.
///
/// The installer from claude.ai, the Microsoft Store and winget all install
/// Claude as an MSIX package, and Windows silently redirects the packaged
/// app's `%APPDATA%` / `%LOCALAPPDATA%` writes into
/// `%LOCALAPPDATA%\Packages\Claude_<publisher-hash>\LocalCache\{Roaming,Local}`
/// (`<pkg>\LocalCache\Roaming\Claude\claude-code-sessions`: 5 levels down).
/// meld is not packaged, so it has to look there itself. Newer MSIX builds
/// (seen on 5.1231) opt out of that and keep their data in
/// `%LOCALAPPDATA%\Claude-Data\claude_<publisher-hash>\claude-code-sessions`
/// (3 levels down). The older non-MSIX installer used the real `%APPDATA%` /
/// `%LOCALAPPDATA%` (2 levels down).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_bases(local: &Path, roaming: &Path) -> Vec<(PathBuf, usize)> {
    vec![(local.join("Packages"), 5), (local.to_path_buf(), 3), (roaming.to_path_buf(), 3)]
}

/// Where a Windows store would be created when none exists yet: for the
/// installed Claude package (preferring the known `Claude_<hash>` name), the
/// current build's `%LOCALAPPDATA%\Claude-Data\claude_<hash>`; else the old
/// installer's `%APPDATA%\Claude`. Only used for the error message.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_expected_store(local: &Path, roaming: &Path) -> PathBuf {
    let package = std::fs::read_dir(local.join("Packages")).ok().and_then(|d| {
        d.flatten()
            .filter(|e| claude_named(&e.file_name()))
            .map(|e| e.path())
            .min_by_key(|p| {
                let n = p.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
                (!n.starts_with("claude_"), n)
            })
    });
    match package.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())) {
        Some(name) => local.join("Claude-Data").join(name).join("claude-code-sessions"),
        None => roaming.join("Claude").join("claude-code-sessions"),
    }
}

/// `%LOCALAPPDATA%` and `%APPDATA%`, falling back to the profile's AppData.
#[cfg(target_os = "windows")]
fn windows_dirs() -> (PathBuf, PathBuf) {
    let profile = home_dir().join("AppData");
    let dir = |var: &str, fallback: &str| {
        std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| profile.join(fallback))
    };
    (dir("LOCALAPPDATA", "Local"), dir("APPDATA", "Roaming"))
}

/// Where Claude's app data can live on this platform, each with how deep a
/// store sits in the known layout.
fn search_bases() -> Vec<(PathBuf, usize)> {
    #[cfg(target_os = "windows")]
    {
        let (local, roaming) = windows_dirs();
        let mut b = windows_bases(&local, &roaming);
        // %APPDATA% can be overridden in a shell (MCP setups do this):
        // also look in the profile's own AppData.
        let profile = home_dir().join("AppData");
        b.extend(windows_bases(&profile.join("Local"), &profile.join("Roaming")));
        b
    }
    #[cfg(not(target_os = "windows"))]
    {
        let base = if cfg!(target_os = "macos") {
            home_dir().join("Library/Application Support")
        } else {
            home_dir().join(".config")
        };
        vec![(base, 2)]
    }
}

/// Where the store would be created when none exists yet.
fn expected_store() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let (local, roaming) = windows_dirs();
        windows_expected_store(&local, &roaming)
    }
    #[cfg(not(target_os = "windows"))]
    {
        search_bases()[0].0.join("Claude").join("claude-code-sessions")
    }
}

/// Every `claude-code-sessions` store under `bases`. When the known layout
/// holds no conversations (nothing found, or only empty leftovers), also
/// look `DEEP_EXTRA` levels deeper, in case a build nests it differently.
fn search_stores(bases: &[(PathBuf, usize)]) -> Vec<PathBuf> {
    let search = |extra: usize| -> Vec<PathBuf> {
        bases.iter().flat_map(|(b, d)| find_stores(b, d + extra)).collect()
    };
    let mut found = search(0);
    if !found.iter().any(|s| session_stats(s).1.is_some()) {
        found.extend(search(DEEP_EXTRA));
    }
    dedup(found)
}

/// Folders laid out like a session store (`<root>/<account>/<org>/local_*.json`)
/// under a Claude-named folder but not named `claude-code-sessions` — e.g. an
/// older build's `local-agent-mode-sessions`. Never used on their own (on
/// current builds that folder holds other sessions); `meld doctor` lists them
/// so the user can point `sessions_root` at the right one.
fn find_lookalikes(bases: &[(PathBuf, usize)]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for (base, depth) in bases {
        let mut walk = walkdir::WalkDir::new(base)
            .min_depth(1)
            .max_depth(depth + DEEP_EXTRA + 1)
            .follow_links(true)
            .sort_by_file_name()
            .into_iter();
        while let Some(entry) = walk.next() {
            let Ok(e) = entry else { continue };
            let name = e.file_name();
            if e.file_type().is_dir() {
                if (e.depth() == 1 && !claude_named(name))
                    || name.eq_ignore_ascii_case("claude-code-sessions")
                {
                    walk.skip_current_dir();
                }
                continue;
            }
            let session = name
                .to_str()
                .is_some_and(|n| n.starts_with("local_") && n.ends_with(".json"));
            // file -> org -> account -> store root
            if let Some(root) = e.path().ancestors().nth(3).filter(|_| session && e.depth() >= 4) {
                if !found.iter().any(|f| f == root) {
                    found.push(root.to_path_buf());
                }
            }
        }
    }
    dedup(found)
}

/// Lookalike stores on this machine (see `find_lookalikes`), with their
/// conversation count and newest write, for `meld doctor`.
pub fn lookalike_stores() -> Vec<(PathBuf, usize, Option<SystemTime>)> {
    find_lookalikes(&search_bases())
        .into_iter()
        .map(|c| {
            let (n, t) = session_stats(&c);
            (c, n, t)
        })
        .collect()
}

/// Drop repeats, including one store reached twice through a junction or
/// symlink (canonical path), keeping the first spelling.
fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    paths
        .into_iter()
        .filter(|p| {
            let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            let new = !seen.contains(&key);
            if new {
                seen.push(key);
            }
            new
        })
        .collect()
}

/// Every place Claude may keep its session store on this platform: the
/// stores found first, then where it would be created. Never empty.
fn candidates_sessions_roots() -> Vec<PathBuf> {
    let mut c = search_stores(&search_bases());
    c.push(expected_store());
    dedup(c)
}

/// Unique conversations (`local_<chat>.json` names) under
/// `<root>/<account>/<org>/`, and when the newest was written. A store that
/// only holds a stub (e.g. `profile-origin.json`) has none and must not
/// shadow a populated one.
fn session_stats(root: &Path) -> (usize, Option<SystemTime>) {
    let mut chats = std::collections::HashSet::new();
    let mut latest = None;
    let Ok(accounts) = std::fs::read_dir(root) else {
        return (0, None);
    };
    for acct in accounts.flatten() {
        let Ok(orgs) = std::fs::read_dir(acct.path()) else {
            continue;
        };
        for org in orgs.flatten() {
            let Ok(files) = std::fs::read_dir(org.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name();
                let Some(n) = name.to_str() else { continue };
                if n.starts_with("local_") && n.ends_with(".json") {
                    let t = f.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
                    latest = latest.max(Some(t));
                    chats.insert(name);
                }
            }
        }
    }
    (chats.len(), latest)
}

/// Every session store on this machine that exists, with its number of
/// conversations and newest write (for `meld doctor`).
pub fn session_stores() -> Vec<(PathBuf, usize, Option<SystemTime>)> {
    candidates_sessions_roots()
        .into_iter()
        .filter(|c| c.is_dir())
        .map(|c| {
            let (n, t) = session_stats(&c);
            (c, n, t)
        })
        .collect()
}

/// The store Claude uses now: the populated candidate with the most recently
/// written conversation (after a reinstall — say the old installer replaced
/// by the MSIX one — the old store lingers but stops changing; ties go to the
/// earlier candidate). Otherwise the first candidate that exists at all.
fn pick_sessions_root(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .rev() // max_by_key keeps the last of equal maxima
        .filter_map(|c| session_stats(c).1.map(|t| (t, c)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, c)| c.clone())
        .or_else(|| candidates.iter().find(|c| c.exists()).cloned())
}

/// Platform default for the Claude Code desktop session store.
fn default_sessions_root() -> PathBuf {
    let candidates = candidates_sessions_roots();
    pick_sessions_root(&candidates).unwrap_or_else(|| candidates[0].clone())
}

pub fn config_path() -> PathBuf {
    home_dir().join(".meld/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meld-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write `<root>/acct/org/local_<chat>.json` with the given mtime.
    fn session(root: &Path, chat: &str, mtime: i64) {
        let org = root.join("acct").join("org");
        fs::create_dir_all(&org).unwrap();
        let f = org.join(format!("local_{chat}.json"));
        fs::write(&f, "{}").unwrap();
        filetime::set_file_mtime(&f, filetime::FileTime::from_unix_time(mtime, 0)).unwrap();
    }

    #[test]
    fn stub_only_store_does_not_shadow_populated_one() {
        let base = tmp_dir("prefers_populated");
        let stub = base.join("stub");
        fs::create_dir_all(&stub).unwrap();
        fs::write(stub.join("profile-origin.json"), "{}").unwrap();
        let real = base.join("real");
        session(&real, "x", 1_000);
        assert_eq!(pick_sessions_root(&[stub.clone(), real.clone()]), Some(real));
        // Nothing populated: the first candidate that exists.
        let missing = base.join("missing");
        assert_eq!(pick_sessions_root(&[missing, stub.clone()]), Some(stub));
    }

    #[test]
    fn newest_store_wins_and_ties_go_to_the_first() {
        let base = tmp_dir("newest_wins");
        let (old, new) = (base.join("old"), base.join("new"));
        session(&old, "x", 1_000);
        session(&new, "x", 2_000);
        assert_eq!(pick_sessions_root(&[old.clone(), new.clone()]), Some(new.clone()));
        session(&old, "x", 2_000);
        assert_eq!(pick_sessions_root(&[old.clone(), new]), Some(old));
    }

    #[test]
    fn session_stats_counts_unique_chats_only() {
        let base = tmp_dir("stats");
        let org = base.join("acct").join("org");
        fs::create_dir_all(&org).unwrap();
        fs::write(org.join("scheduled-tasks.json"), "[]").unwrap();
        fs::write(org.join("deleted_x"), "marker").unwrap();
        assert_eq!(session_stats(&base), (0, None));
        assert_eq!(session_stats(&base.join("nope")), (0, None));

        session(&base, "x", 1_000);
        session(&base, "y", 3_000);
        let org2 = base.join("acct").join("org2");
        fs::create_dir_all(&org2).unwrap();
        fs::copy(org.join("local_x.json"), org2.join("local_x.json")).unwrap();
        let (n, latest) = session_stats(&base);
        assert_eq!(n, 2, "the same chat in two orgs is one conversation");
        assert!(latest.is_some());
    }

    #[test]
    fn windows_search_finds_stores_wherever_the_package_puts_them() {
        let base = tmp_dir("msix");
        let (local, roaming) = (base.join("Local"), base.join("Roaming"));
        let pkgs = local.join("Packages");
        // Fresh MSIX install, no sessions yet: the default is inside the
        // package, even with another Claude-named package sorting first.
        fs::create_dir_all(pkgs.join("Claude_pzs8sxrjxfjjc").join("LocalCache")).unwrap();
        fs::create_dir_all(pkgs.join("Anthropic.Claude_1234abcd")).unwrap();
        let bases = windows_bases(&local, &roaming);
        assert!(search_stores(&bases).is_empty());
        let pkg = pkgs.join("Claude_pzs8sxrjxfjjc");
        let msix = pkg.join("LocalCache").join("Roaming").join("Claude").join("claude-code-sessions");
        // Where Claude 5.x keeps it (verified on a real install): outside the
        // package, under the lower-cased package family name.
        let data = local.join("Claude-Data").join("claude_pzs8sxrjxfjjc").join("claude-code-sessions");
        assert_eq!(windows_expected_store(&local, &roaming), data);

        // Stores in the usual place, under another hash, in LocalState, a
        // 3P/beta channel and the old installer's folder are all found.
        let other = pkgs.join("Anthropic.Claude_1234abcd").join("LocalState").join("claude-code-sessions");
        let beta = local.join("Claude Beta").join("claude-code-sessions");
        let old = roaming.join("Claude").join("claude-code-sessions");
        let p3 = pkg.join("LocalCache").join("Local").join("Claude-3p").join("claude-code-sessions");
        // Not Claude's: never searched.
        let unrelated = pkgs.join("Microsoft.WindowsTerminal_8wekyb3d8bbwe").join("LocalState").join("claude-code-sessions");
        for d in [&msix, &other, &beta, &old, &p3, &unrelated] {
            fs::create_dir_all(d).unwrap();
        }
        session(&data, "x", 1_000);
        let c = search_stores(&bases);
        for d in [&data, &msix, &other, &beta, &old, &p3] {
            assert!(c.contains(d), "missing {}", d.display());
        }
        assert!(!c.contains(&unrelated));
        assert_eq!(c.len(), 6);
        assert_eq!(pick_sessions_root(&c), Some(data), "the only store with conversations");
    }

    #[test]
    fn deeper_store_is_found_only_when_the_usual_layout_has_none() {
        let base = tmp_dir("deep");
        let (local, roaming) = (base.join("Local"), base.join("Roaming"));
        let deep = local
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("Claude")
            .join("Partitions")
            .join("main")
            .join("claude-code-sessions");
        fs::create_dir_all(&deep).unwrap();
        let bases = windows_bases(&local, &roaming);
        assert_eq!(search_stores(&bases), vec![deep.clone()]);
        // An empty leftover in the usual place doesn't stop the deeper look...
        let usual = roaming.join("Claude").join("claude-code-sessions");
        fs::create_dir_all(&usual).unwrap();
        assert_eq!(search_stores(&bases), vec![usual.clone(), deep]);
        // ...but a usual store with conversations does.
        session(&usual, "x", 1_000);
        assert_eq!(search_stores(&bases), vec![usual]);
    }

    #[test]
    fn lookalike_stores_are_reported_but_never_picked() {
        let base = tmp_dir("lookalike");
        let roaming = base.join("Roaming");
        let legacy = roaming.join("Claude").join("local-agent-mode-sessions");
        session(&legacy, "x", 1_000);
        let real = roaming.join("Claude").join("claude-code-sessions");
        session(&real, "y", 1_000);
        // Session-like files outside a Claude folder are not Claude's.
        session(&roaming.join("Other").join("stuff"), "z", 1_000);
        let bases = vec![(roaming.clone(), 2)];
        assert_eq!(find_lookalikes(&bases), vec![legacy]);
        assert_eq!(search_stores(&bases), vec![real]);
    }

    #[cfg(unix)]
    #[test]
    fn dedup_merges_a_store_reached_through_a_link() {
        let base = tmp_dir("dedup_link");
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let missing = base.join("missing");
        assert_eq!(
            dedup(vec![real.clone(), link, missing.clone(), missing.clone()]),
            vec![real, missing]
        );
    }
}

impl Config {
    /// Load from ~/.meld/config.toml, or defaults if absent.
    pub fn load() -> Result<Config> {
        let path = config_path();
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading config {}", path.display()))?;
            let mut cfg: Config = toml::from_str(&raw)
                .with_context(|| format!("parsing config {}", path.display()))?;
            // A pinned folder that no longer exists (e.g. Claude reinstalled as
            // the Windows MSIX package, which stores sessions elsewhere) can't
            // work; use the detected store instead of failing every run.
            if !cfg.sessions_root.exists() {
                let detected = default_sessions_root();
                if detected.exists() {
                    crate::logging::warn(&format!(
                        "sessions_root {} (set in {}) does not exist; using {} instead",
                        cfg.sessions_root.display(),
                        path.display(),
                        detected.display()
                    ));
                    cfg.sessions_root = detected;
                }
            }
            Ok(cfg)
        } else {
            Ok(Config::default())
        }
    }

    /// Write the default config to ~/.meld/config.toml (no overwrite).
    pub fn init() -> Result<PathBuf> {
        let path = config_path();
        if path.exists() {
            anyhow::bail!("config already exists at {}", path.display());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cfg = Config::default();
        // Leave the sessions folder to auto-detection (commented out) so a
        // reinstall that moves it doesn't leave a stale path pinned.
        let body = toml::to_string_pretty(&cfg)?.replacen("sessions_root =", "# sessions_root =", 1);
        std::fs::write(&path, body)?;
        Ok(path)
    }

    pub fn policy(&self) -> Policy {
        Policy {
            deletes: if self.propagate_deletes == Some(true) {
                DeletedChats::DeleteEverywhere
            } else {
                self.deleted_chats
            },
            propagate_updates: self.propagate_updates,
        }
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.state_dir.join("manifest.json")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("meld.lock")
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| self.ignore.iter().any(|i| i == n))
            .unwrap_or(false)
    }
}
