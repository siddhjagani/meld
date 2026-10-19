use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "meld",
    version,
    about = "Merge Claude Code sessions across accounts — safely, locally, reversibly"
)]
pub struct Cli {
    /// Verbose per-file output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Machine-readable JSON output (status/diff)
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scan all account trees and rebuild the manifest
    Scan,
    /// List the Claude accounts and organizations meld found
    Accounts,
    /// Show account roots, file counts, coverage and conflicts
    Status,
    /// Show per-account differences (what sync would copy)
    Diff,
    /// Make every account and organization hold the same conversations:
    /// copy new ones and spread the latest version
    Sync {
        /// Show what would happen without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Skip the pre-sync snapshot (not recommended)
        #[arg(long)]
        no_backup: bool,
        /// Quit Claude first (it saves its state), sync, then reopen it so
        /// it shows the synced conversations (macOS)
        #[arg(long)]
        restart_claude: bool,
    },
    /// Watch for changes and sync automatically
    Watch {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_backup: bool,
    },
    /// Snapshot every account tree now
    Backup,
    /// Roll back from a snapshot (newest by default)
    Restore {
        /// Snapshot name (see `meld restore --list`)
        #[arg(long)]
        snapshot: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// List available snapshots instead of restoring
        #[arg(long)]
        list: bool,
    },
    /// Run `meld watch` in the background at login (macOS)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Check paths, permissions and index health
    Doctor,
    /// Config management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Start background sync now and at every login
    Install,
    /// Stop background sync and remove it from login
    Uninstall,
    /// Show whether background sync is running
    Status,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Write the default config to ~/.meld/config.toml
    Init,
    /// Print the effective config
    Show,
}
