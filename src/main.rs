use meld::{accounts, backup, claude_app, cli, config, diff, discover, errors, lock, logging, manifest, restore, scan, service, sync, watch};

use clap::Parser;
use cli::{Cli, Command, ConfigAction, ServiceAction};
use config::Config;
use errors::Result;
use manifest::Manifest;

fn main() {
    let cli = Cli::parse();
    logging::set_verbose(cli.verbose);
    logging::set_json(cli.json);
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let cfg = Config::load()?;

    match cli.command {
        Command::Config { action: ConfigAction::Init } => {
            let path = Config::init()?;
            logging::info(&format!("wrote {}", path.display()));
        }
        Command::Config { action: ConfigAction::Show } => {
            println!("{}", toml::to_string_pretty(&cfg)?);
        }
        Command::Scan => {
            let m = do_scan(&cfg)?;
            let unique: std::collections::BTreeSet<_> =
                m.entries.iter().map(|e| e.chat()).collect();
            logging::info(&format!(
                "Found {} accounts with {}.",
                m.account_roots.len(),
                sync::conversations(unique.len())
            ));
            logging::verbose(&format!("index saved to {}", cfg.manifest_path().display()));
        }
        Command::Accounts => {
            let m = do_scan(&cfg)?;
            print_accounts(&m);
        }
        Command::Status => {
            let m = do_scan(&cfg)?;
            let report = diff::compute(&m, cfg.policy());
            if cli.json {
                print_status_json(&m, &report)?;
            } else {
                print_status(&m, &report);
            }
        }
        Command::Diff => {
            let m = do_scan(&cfg)?;
            let report = diff::compute(&m, cfg.policy());
            if cli.json {
                print_status_json(&m, &report)?;
            } else {
                print_diff(&report);
            }
        }
        Command::Sync { dry_run, no_backup, restart_claude } => {
            let _lock = lock::Lock::acquire(&cfg)?;
            let restart = restart_claude && !dry_run;
            if restart {
                claude_app::quit_and_wait()?;
            }
            let result = sync_once(&cfg, dry_run, no_backup);
            if restart {
                // Reopen even if the sync failed, so the user isn't left without the app.
                claude_app::launch()?;
            }
            result?;
        }
        Command::Watch { dry_run, no_backup } => {
            let _lock = lock::Lock::acquire(&cfg)?;
            watch::watch(&cfg, dry_run, no_backup)?;
        }
        Command::Backup => {
            let roots = discover::discover_account_roots(&cfg)?;
            backup::snapshot(&cfg, &roots)?;
        }
        Command::Restore { list: true, .. } => restore::list(&cfg)?,
        Command::Restore { snapshot, dry_run, .. } => {
            let _lock = lock::Lock::acquire(&cfg)?;
            restore::restore(&cfg, snapshot.as_deref(), dry_run)?;
        }
        Command::Service { action: ServiceAction::Install } => service::install()?,
        Command::Service { action: ServiceAction::Uninstall } => service::uninstall()?,
        Command::Service { action: ServiceAction::Status } => service::status()?,
        Command::Doctor => doctor(&cfg)?,
    }
    Ok(())
}

/// One `meld sync`. Files written in the last moments (Claude saving, or
/// just quit) are skipped by the scan; wait for them instead of leaving them
/// out, so a sync right after using Claude includes the latest state.
fn sync_once(cfg: &Config, dry_run: bool, no_backup: bool) -> Result<()> {
    let roots = discover::discover_account_roots(cfg)?;
    let mut m = do_scan(cfg)?;
    for _ in 0..5 {
        if m.skipped_recent == 0 {
            break;
        }
        logging::verbose("waiting for files that are still being written...");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        m = do_scan(cfg)?;
    }
    let report = diff::compute(&m, cfg.policy());
    sync::apply(cfg, &report, dry_run, no_backup)?;
    if !dry_run {
        let m2 = scan::scan(cfg, &roots, &m)?;
        m2.save(&cfg.manifest_path())?;
    }
    Ok(())
}

fn do_scan(cfg: &Config) -> Result<Manifest> {
    let roots = discover::discover_account_roots(cfg)?;
    let previous = Manifest::load_or_default(&cfg.manifest_path());
    let m = scan::scan(cfg, &roots, &previous)?;
    m.save(&cfg.manifest_path())?;
    Ok(m)
}

/// "Account 2 (bac90339)" — stable, human-scannable account label.
fn account_label(root: &std::path::Path) -> String {
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    format!("Account {}", accounts::short(name))
}

fn print_accounts(m: &Manifest) {
    let views = accounts::build(m);
    let now = chrono::Utc::now().timestamp();
    let total: usize = views.iter().map(|a| a.conversations).sum();
    let org_count: usize = views.iter().map(|a| a.orgs.len()).sum();

    println!(
        "{} across {} — {} in total, all merged together.\n",
        sync::conversations(total),
        pluralize(views.len(), "account", "accounts"),
        pluralize(org_count, "organization", "organizations"),
    );

    for a in &views {
        let name = a.root.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        println!(
            "Account {} · {} · last active {}",
            accounts::short(name),
            sync::conversations(a.conversations),
            accounts::humanize_age(a.last_active, now),
        );
        let n = a.orgs.len();
        for (i, org) in a.orgs.iter().enumerate() {
            let branch = if i + 1 == n { "└" } else { "├" };
            let paths = if org.top_paths.is_empty() {
                String::new()
            } else {
                format!("  ({})", org.top_paths.join(", "))
            };
            println!(
                "  {branch} org {}: {} · {}{}",
                accounts::short(&org.id),
                sync::conversations(org.conversations),
                accounts::humanize_age(org.last_active, now),
                paths,
            );
        }
        println!();
    }
    println!("meld keeps all of these in sync with each other.");
}

fn pluralize(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn print_status(m: &Manifest, report: &diff::DiffReport) {
    let unique: std::collections::BTreeSet<_> =
        m.entries.iter().map(|e| e.chat()).collect();
    let views = accounts::build(m);
    let org_count: usize = views.iter().map(|a| a.orgs.len()).sum();
    println!(
        "You have {} across {} ({}).\n",
        sync::conversations(unique.len()),
        pluralize(views.len(), "account", "accounts"),
        pluralize(org_count, "organization", "organizations"),
    );
    for a in &views {
        let name = a.root.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        print!("  {}: {}", accounts::short(name), sync::conversations(a.conversations));
        if a.orgs.len() > 1 {
            print!(" across {}", pluralize(a.orgs.len(), "organization", "organizations"));
        }
        println!();
    }
    println!("\n(Run `meld accounts` for more detail on each one.)");
    println!();
    if report.is_empty() {
        println!("All accounts are up to date.");
    } else {
        println!(
            "Waiting to sync: {} to copy, {} to update, {} to delete. Run `meld sync` to update all accounts.",
            report.copies.len(),
            report.updates.len(),
            report.deletes.len()
        );
    }
    if !report.conflicts.is_empty() {
        let n = report.conflicts.len();
        println!(
            "\n{} {} in two versions (both kept, nothing was changed):",
            sync::conversations(n),
            if n == 1 { "exists" } else { "exist" }
        );
        for cf in &report.conflicts {
            let newest = cf.variants.iter().max_by_key(|v| v.mtime);
            if let Some(v) = newest {
                println!("  - {}", sync::describe(v));
            }
        }
        println!("These are usually the same chat continued in different accounts.");
    }
}

fn print_diff(report: &diff::DiffReport) {
    if report.is_empty() {
        println!("Nothing to sync — every account has the same conversations.");
        return;
    }
    // Group pending changes by the account they land in.
    let mut by_dest: std::collections::BTreeMap<&std::path::Path, Vec<String>> =
        std::collections::BTreeMap::new();
    for c in &report.copies {
        by_dest
            .entry(c.dest_root.as_path())
            .or_default()
            .push(format!("+ copy    {}", sync::describe(&c.source)));
    }
    for u in &report.updates {
        by_dest
            .entry(u.target.account_root.as_path())
            .or_default()
            .push(format!("~ update  {}", sync::describe(&u.source)));
    }
    for d in &report.deletes {
        by_dest
            .entry(d.target.account_root.as_path())
            .or_default()
            .push(format!("- delete  {}", sync::describe(&d.target)));
    }
    for (dest, lines) in by_dest {
        println!("{}:", account_label(dest));
        for l in lines {
            println!("  {l}");
        }
    }
    println!("\nRun `meld sync` to apply, or `meld sync --dry-run` to preview again.");
}

fn print_status_json(m: &Manifest, report: &diff::DiffReport) -> Result<()> {
    let out = serde_json::json!({
        "sessions_root": m.sessions_root,
        "account_roots": m.account_roots,
        "total_files": m.entries.len(),
        "pending_copies": report.copies.iter().map(|c| serde_json::json!({
            "relative_path": c.source.relative_path,
            "dest_root": c.dest_root,
        })).collect::<Vec<_>>(),
        "pending_updates": report.updates.iter().map(|u| serde_json::json!({
            "relative_path": u.target.relative_path,
            "dest_root": u.target.account_root,
        })).collect::<Vec<_>>(),
        "pending_deletes": report.deletes.iter().map(|d| serde_json::json!({
            "relative_path": d.target.relative_path,
            "dest_root": d.target.account_root,
        })).collect::<Vec<_>>(),
        "conflicts": report.conflicts.iter().map(|c| serde_json::json!({
            "relative_path": c.relative_path,
            "variants": c.variants.len(),
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn doctor(cfg: &Config) -> Result<()> {
    let mut ok = true;
    let check = |pass: bool, msg: &str| {
        println!("{} {msg}", if pass { "✓" } else { "✗" });
        pass
    };

    let found = cfg.sessions_root.exists();
    ok &= check(
        found,
        if found {
            "Claude Code is installed and has session data"
        } else {
            "Could not find Claude Code session data — is the desktop app installed?"
        },
    );
    logging::verbose(&format!("sessions root: {}", cfg.sessions_root.display()));

    if found {
        match discover::discover_account_roots(cfg) {
            Ok(roots) => {
                let n = roots.len();
                ok &= check(
                    n > 0,
                    &if n > 0 {
                        format!("{n} account{} found", if n == 1 { "" } else { "s" })
                    } else {
                        "No accounts found yet — sign in to Claude Code first".into()
                    },
                );
                if let Some(r) = roots.first() {
                    let probe = r.join(".meld-doctor-probe");
                    let writable = std::fs::write(&probe, b"probe").is_ok();
                    let _ = std::fs::remove_file(&probe);
                    ok &= check(
                        writable,
                        if writable {
                            "Meld is allowed to read and update your sessions"
                        } else if cfg!(target_os = "macos") {
                            "Meld doesn't have permission to update your sessions — check Full Disk Access in System Settings"
                        } else {
                            "Meld doesn't have permission to update your sessions — check the folder's permissions"
                        },
                    );
                }
            }
            Err(e) => {
                ok &= check(false, &format!("Could not look for accounts: {e}"));
            }
        }
    }

    let state_ok = std::fs::create_dir_all(&cfg.state_dir).is_ok();
    ok &= check(
        state_ok,
        if state_ok {
            "Backups and settings are ready"
        } else {
            "Could not prepare meld's own folder"
        },
    );
    logging::verbose(&format!("meld folder: {}", cfg.state_dir.display()));

    let snaps = backup::list_snapshots(cfg)?.len();
    if snaps > 0 {
        println!(
            "✓ {snaps} backup{} available (undo any sync with `meld restore`)",
            if snaps == 1 { "" } else { "s" }
        );
    }

    if ok {
        println!("\nEverything looks good. Run `meld sync` whenever you like.");
        Ok(())
    } else {
        anyhow::bail!("some checks failed (see above)")
    }
}
