use thiserror::Error;

#[derive(Error, Debug)]
pub enum MeldError {
    #[error("Claude Code session folder not found: {0}\nOpen the Claude desktop app and start a Claude Code session first, or set `sessions_root` in ~/.meld/config.toml. Run `meld doctor` for details.")]
    RootNotFound(String),

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
