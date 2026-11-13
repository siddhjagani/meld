use crate::config::Config;
use crate::errors::{MeldError, Result};
use std::path::PathBuf;

/// Find every account root: the immediate subdirectories of the sessions
/// root. Folder names are not trusted for anything beyond identity — content
/// decides everything downstream.
pub fn discover_account_roots(cfg: &Config) -> Result<Vec<PathBuf>> {
    let root = &cfg.sessions_root;
    if !root.exists() {
        let searched: Vec<String> = crate::config::search_locations()
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect();
        return Err(MeldError::RootNotFound {
            path: root.display().to_string(),
            searched: searched.join("\n"),
            config: crate::config::config_path().display().to_string(),
        }
        .into());
    }
    let mut roots: Vec<PathBuf> = std::fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| !cfg.is_ignored(p))
        .collect();
    roots.sort();
    Ok(roots)
}
