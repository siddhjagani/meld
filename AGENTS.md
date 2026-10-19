# Meld — agent notes

Rust CLI that mirrors Codex desktop session JSONs across account trees
under `~/Library/Application Support/Codex/Codex-sessions/`.
Read `context.md` first for current state and decisions.

## Invariants (do not break)

1. Never delete user files — not in sync, not in restore, not in prune of
   session trees. Only `~/.meld/backups` snapshots may be pruned.
2. Never overwrite an existing session file. Same relative path + different
   hash = conflict, surfaced to the user, both variants kept.
3. Every write is temp-file + fsync + rename (`.meld-tmp` suffix — the
   watcher ignores that suffix to avoid feedback loops).
4. `meld sync` snapshots all trees before its first write unless `--no-backup`.
5. Content decides identity, not folder names. The sync key is the path
   relative to the account root; equality is sha256.
6. Never decrypt Codex's OAuth token or read Keychain secrets to show
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
- Pipeline: `discover` (account roots) → `scan` (walk+hash, reuses hashes when
  size+mtime unchanged) → `diff` (union-mirror plan) → `sync` (apply).
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
`~/Library/Application Support/Codex` path.
