use thiserror::Error;

#[derive(Error, Debug)]
pub enum MeldError {
    #[error("no Claude Code session folder found. meld searched every Claude folder under:\n{searched}\nOpen the Claude desktop app and start a Claude Code session first, or set `sessions_root` in {config} (expected: {path}). Run `meld doctor` for details.")]
    RootNotFound {
        /// The folder meld expected (configured or detected).
        path: String,
        /// The searched locations, one indented line each.
        searched: String,
        config: String,
    },

    #[error("another meld instance is running (lock file: {0}). If background sync is on (`meld service status`), syncing already happens automatically; to restore, turn it off first with `meld service uninstall`.")]
    Locked(String),

    #[error("no snapshot available to restore")]
    NoSnapshot,

    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = anyhow::Result<T>;
