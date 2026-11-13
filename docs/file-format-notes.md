# Claude Code desktop session store — observed format

Verified on macOS, Claude desktop, 2026-07-07 (this machine). The Windows
path (`%APPDATA%\Claude\claude-code-sessions`) mirrors the same per-account /
per-workspace layout; the schema is identical across platforms.

## Layout

```
<sessions-root>/
├─ <account-uuid>/                 # one per Claude login (an email/account)
│  ├─ .DS_Store                    # Finder noise, ignored (macOS)
│  └─ <organization-uuid>/         # a personal space, a team org, etc.
│     └─ local_<session-uuid>.json # one file per session
```

Corrected 2026-07-17: the second level is per-**organization**, not a
shared "workspace". One account (login) can have more than one organization
folder nested under it — e.g. a personal space and a team it belongs to —
or just one. Two accounts can also happen to contain the same organization
(the org id repeats under each account it's shared with), which is exactly
the fragmentation meld fixes: `<account>/<org>/<chat>.json` is the sync key,
and meld mirrors it wherever that org appears.

Confusingly, organization ids and account ids are drawn from the same UUID
namespace, so an organization id can be identical to some other, unrelated
account's id purely by coincidence. Confirmed on this machine: one
account's org folder had the same UUID as a sibling top-level account. This
does not cause incorrect merging — meld keys off the full relative path
(`<account>/<org>/<chat>`), never off an org id in isolation — but it can
look alarming when eyeballing the raw folders. `meld accounts` exists
specifically so you don't have to eyeball them.

Claude does not expose the human-readable identity (email, organization
display name, plan) anywhere meld can safely read: the OAuth token is
Electron `safeStorage`-encrypted with its key in the macOS Keychain, and
`lastKnownAccountUuid` in `config.json` isn't guaranteed to match any
session folder at all. Meld will not decrypt that token or touch Keychain
secrets. Instead it fingerprints each account/org from what's already on
disk — conversation count, last-active time (max mtime), and the most
common `cwd` values — good enough to tell folders apart without ever
handling a credential.

`<sessions-root>` per platform (see `config::default_sessions_root`):

- macOS:   `~/Library/Application Support/Claude/claude-code-sessions`
- Windows, current MSIX build (verified 2026-11-12 on Claude 5.1231.2.0,
  package `Claude_pzs8sxrjxfjjc`):
  `%LOCALAPPDATA%\Claude-Data\claude_pzs8sxrjxfjjc\claude-code-sessions` —
  outside the package container, under the lower-cased package family name
  (the bundled Claude Code CLI lives next to it in `claude-code\<version>\`).
  The package's own `LocalCache\Roaming\Claude` existed but was empty.
- Windows, earlier MSIX builds:
  `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\claude-code-sessions`
- Windows, old non-MSIX installer: `%APPDATA%\Claude\claude-code-sessions`
- Linux:   `~/.config/Claude/claude-code-sessions` (untested — no Linux desktop)

Observed on this machine: 3 accounts; one of them nests 2 organizations
(21 and 1 conversations respectively), the other two have 1 organization
each. Session files overlap partially between accounts sharing an
organization — the fragmentation meld fixes.

## Session file schema (top-level keys observed)

Single JSON object. Keys seen:

`sessionId`, `cliSessionId`, `cwd`, `originCwd`, `lastFocusedAt`,
`createdAt`, `lastActivityAt`, `model`, `effort`, `isArchived`, `title`,
`titleSource`, `permissionMode`, `enabledMcpTools`,
`remoteMcpServersConfig`, `chromePermissionMode`, `error`, `errorAt`,
`alwaysAllowedReasons`, `sessionPermissionUpdates`

- `sessionId` matches the `<session-uuid>` in the filename → meld prefers the
  JSON field and falls back to the filename.
- Timestamps appear to be epoch-based / ISO fields; meld does not parse them
  (mtime is used for newest-wins).

## Caveats

- Schema is undocumented and may drift between desktop versions. Meld treats
  files as opaque bytes for sync purposes; only `sessionId` is read, and its
  absence is tolerated.
- There is a sibling `local-agent-mode-sessions/` folder — intentionally NOT
  synced by meld (out of scope until its semantics are understood).

## First-party vs third-party stores

Claude stores its session data under two possible roots, and meld scans both
(`config::default_sessions_root`):

- **First-party (1P):** the plain Claude install, one store per platform
  (e.g. `%APPDATA%\Claude\claude-code-sessions` on Windows,
  `~/Library/Application Support/Claude/claude-code-sessions` on macOS).
- **Third-party (3P):** Claude's inference profile store, next to the 1P root
  (`.../Claude-3p/claude-code-sessions`). It exists on Windows, macOS and
  Linux alike.

On Windows the current Claude build is an MSIX package. Windows redirects the
packaged app's `%APPDATA%` / `%LOCALAPPDATA%` writes into
`%LOCALAPPDATA%\Packages\Claude_<publisher-hash>\LocalCache\{Roaming,Local}`,
which an unpackaged tool like meld does not see through `%APPDATA%` (verified
via anthropics/claude-code#25579 and #26073; hash `pzs8sxrjxfjjc` today).

Because that layout is observed, not documented, meld searches instead of
assuming (`config::find_stores`): every `claude-code-sessions` folder inside a
folder whose name contains `claude` or `anthropic` (any package hash or name,
`LocalCache` or `LocalState`, `Claude`, `Claude-3p`, a beta channel, ...),
bounded in depth so a large AppData stays fast:

- `%LOCALAPPDATA%\Packages\<Claude-named>\…` — up to 5 levels down
- `%LOCALAPPDATA%\<Claude-named>\…` and `%APPDATA%\<Claude-named>\…` — 3
  levels (covers `Claude-Data\claude_<hash>\claude-code-sessions`)
- the same under `%USERPROFILE%\AppData`, in case `%APPDATA%` was overridden

On macOS/Linux the same search runs over `~/Library/Application Support` /
`~/.config`. Stores reached twice (junction/symlink) are counted once. If
none of the stores found holds a conversation (nothing found, or only empty
leftovers), it also looks 4 levels deeper (a build that nests the folder
differently).

Builds before ~Feb 2026 reportedly used `local-agent-mode-sessions` for the
same files; on current builds that sibling holds other sessions, so meld
never syncs it by itself. `meld doctor` lists any folder laid out like a
store (`<root>/<account>/<org>/local_*.json`) under another name and, when
no real store exists, prints the `sessions_root` line to use it.

Resolution (`config::pick_sessions_root`):

1. Among stores holding `local_<chat>.json` files (a stub holding only
   `profile-origin.json` doesn't count), the one with the most recently
   written session — after a reinstall the old store lingers but stops
   changing. Ties go to the earlier candidate.
2. Otherwise the first store that exists at all.
3. Otherwise where the installed build would create it (inside the Claude
   package if one is installed, else `%APPDATA%\Claude\…`) — used for the
   error message.

`meld doctor` lists every store found with its conversation count and last
write, marks the one in use, and warns when more than one holds
conversations (meld syncs only one).

A `sessions_root` pinned in `config.toml` wins, unless it doesn't exist —
then meld warns and uses the detected store. `meld config init` writes it
commented out so it isn't pinned by accident.

This means a user can legitimately have history in both stores and meld will
index whichever one is populated, never silently shadowing a real store with an
empty stub.

