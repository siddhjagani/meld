# Meld — agent notes

Rust CLI that mirrors Claude Code desktop session JSONs across account trees
under `~/Library/Application Support/Claude/claude-code-sessions/`.
Read `context.md` first for current state and decisions.

## Invariants (do not break)

1. Only delete a session file to mirror a deletion Claude made in another
   account (`deleted_chats = "delete_everywhere"`, opt-in; default
   `come_back` never deletes): write the `deleted_<chat>` marker the way
   Claude does, then remove `local_<chat>.json`. Restore may remove deletion
   markers to un-delete. Nothing else is ever deleted from session trees.
2. Only overwrite a session file with a strictly newer copy of the same
   conversation (`propagate_updates`, on by default, can be turned off). Equal mtimes + different content =
   conflict, surfaced to the user, both variants kept. Skip any target whose
   size/mtime moved since the scan. "Newest" means the copy edited since the
   accounts last agreed (`Manifest::base`); mtime alone only decides when no
   agreed version is on record (then transcript lineage `priorCliSessionIds`,
   then `lastActivityAt`, then mtime). A copy that goes back to a version in
   `Manifest::history` is stale (Claude saved its in-memory copy) and is
   repaired, never spread. Updates merge: `scan::LOCAL_KEYS` (org
   connectors, per-account errors, process handles) keep the target's
   values and never count as a difference. A copy edited since the last
   agreement is never deleted by a propagated deletion.
3. Every write is temp-file + fsync + rename (`.meld-tmp` suffix — the
   watcher ignores that suffix to avoid feedback loops).
4. `meld sync` snapshots before its first write unless `--no-backup`: all
   trees for a copy-only sync, or exactly the files it replaces or deletes
   when full sync is on. A restore snapshots the files it replaces or
   removes first, so it can be undone.
5. Content decides identity, not folder names. The sync key is the chat's
   file name (`local_<chat>.json`): every org folder of every account gets
   every chat. Org folders are UUID-named dirs under an account (the app's
   own rule); other folders and Claude's sub-folders are never touched.
6. Never decrypt Claude's OAuth token or read Keychain secrets to show
   email/org names. Account identity in output is always an on-disk
   fingerprint (`src/accounts.rs`), never the real human identity.

## Structure

- `src/lib.rs` re-exports all modules so `tests/` can use them.
- Layout is `<account>/<organization>/local_<chat>.json` — one account
  (login) can nest more than one organization folder. `discover` only sees
  account roots; `FileEntry::organization()` derives the org from the
  relative path. `src/accounts.rs` builds the account→org fingerprint used
  by `meld accounts` and `meld status` (counts, last-active, top cwd paths —
  never email/org name, see invariant 6).
- Pipeline: `discover` (account roots) → `scan` (walk+hash `local_*.json`,
  collect `deleted_*` markers; reuses hashes when size+mtime unchanged) →
  `diff` (copy / update / delete plan) → `sync` (apply). Other files in the
  trees (e.g. `scheduled-tasks.json`) are per-account and never synced.
- Full snapshots (`meld backup`, copy-only syncs; name ends `-full`) have their
  own cap `max_full_snapshots`; per-file snapshots use `max_snapshots`.
- `service` installs `meld watch` as the `com.meld.watch` LaunchAgent (macOS).
- `watch` re-runs the whole pipeline after a debounced event burst.
- State in `~/.meld/` (`config.toml`, `manifest.json`, `backups/`, `meld.lock`).
- Cross-platform (macOS + Windows): session-root default is per-OS in
  `config.rs`; `lock.rs` has cfg(unix)/cfg(windows) process-alive checks
  (no libc crate — tiny extern shims); mtime preservation uses the
  `filetime` crate; renames over existing files must go through
  `manifest::replace_file` (Windows can't rename onto an existing file).

## Gotchas

- `scan` skips files modified <2s ago (`WRITE_QUIET_SECS`) — tests must
  backdate files with `touch -t` or they are invisible to the scanner.
- Copied files inherit the source mtime so newest-wins stays stable.
- Manifest is JSON, not SQLite (deliberate: fewer deps, trees are small).

## Commands

`cargo test` runs integration tests on synthetic temp trees (never the real
sessions folder). `./scripts/smoke-test.sh` builds and end-to-ends against a
sandbox with `HOME` overridden. Never point tests at the real
`~/Library/Application Support/Claude` path.
