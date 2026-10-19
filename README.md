<p align="center">
  <img src="assets/logo.png" alt="Meld logo" width="140">
</p>

<h1 align="center">Meld</h1>

<p align="center">One shared Claude Code chat history across all of your accounts.</p>

---

If you use Claude Code desktop with more than one account, each account keeps
its own separate session history on your Mac. Switch accounts and your recent
conversations seem to disappear. They are still on your machine — just stored
in a different account folder.

Meld fixes this. It finds every account's session folder, works out which
conversations are missing where, and copies them so every account sees the
same complete history. Everything happens locally on your Mac. Nothing is
uploaded anywhere, nothing is ever deleted (unless you opt in to full sync),
and every change can be undone.

If an account belongs to more than one organization (for example a personal
space and a team), meld treats each one as its own history and keeps that
organization in sync across every account that has it.

## Install

Recommended:

```bash
curl -fsSL https://meld.dpdns.org/install.sh | bash
```

With Homebrew:

```bash
brew install siddhjagani/tap/meld
```

On Windows (PowerShell, no administrator needed):

```powershell
irm https://meld.dpdns.org/install.ps1 | iex
```

macOS (Apple Silicon and Intel) and Windows are supported.

## Get started in three commands

```bash
meld doctor          # 1. confirm meld can see your session folders
meld sync --dry-run  # 2. preview exactly what would be copied
meld sync            # 3. bring every account up to date
```

That is the whole workflow. Run `meld sync` again any time; if there is
nothing to do, it says so and touches nothing.

## Keep accounts in sync automatically

```bash
meld watch
```

Leave this running and meld will notice new conversations as they appear and
copy them across your accounts within a couple of seconds. Press Ctrl-C to
stop. Signing into a brand-new account later requires no setup — meld detects
the new account folder on its own and fills it with your existing history.

## Optional: full sync (updates and deletions follow you)

By default meld only adds: new conversations are copied to every account and
nothing is ever replaced or deleted. Only conversations are synced —
per-account files such as `scheduled-tasks.json` stay with their account.

If you want every account to stay identical, not just complete, turn on full
sync in `~/.meld/config.toml` (create it with `meld config init`):

```toml
propagate_updates = true   # continued chats: newest copy replaces older ones
propagate_deletes = true   # deleted chats: deleted in every account
```

- **Continued conversations**: the newest copy replaces the older copies in
  the other accounts, so you can pick up where you left off after switching.
- **Deleted conversations** are deleted in every account — delete a chat
  while signed in to A, and it is gone when you sign in to B. Meld deletes it
  the same way Claude does (a `deleted_<chat>` marker replaces the file), so a
  deleted conversation is never copied back.

Every copy that is replaced or deleted is backed up first; `meld restore`
puts it back.

## Run it in the background (macOS)

```bash
meld service install     # start background sync now and at every login
meld service status      # is it running?
meld service uninstall   # turn it off
```

This runs `meld watch` as a login item, restarts it if it stops, and logs to
`~/Library/Logs/meld.log`. While it runs, `meld sync` isn't needed (and
`meld restore` asks you to turn the service off first).

## Check where things stand

```bash
meld status
```

Shows each account, how many conversations it has, whether anything is
pending, and any conflicts (see below). Add `--json` if you want
machine-readable output.

```bash
meld diff
```

Lists the exact files a sync would copy, and where they would go.

```bash
meld accounts
```

Meld cannot see your email address or organization name — Claude keeps those
behind an encrypted login token, and meld never touches that. Instead this
lists every account (and every organization inside it) with what's on disk:
how many conversations, when it was last active, and which projects it
worked on. Use it to tell your accounts apart, and to confirm meld found all
of them — including a single login that belongs to more than one
organization.

## Undo a sync

Before meld writes anything, it saves a snapshot: of all your session
folders, or — when full sync replaces or deletes files — of exactly those
files. To go back:

```bash
meld restore              # undo the most recent change
meld restore --list       # see every snapshot
meld restore --snapshot <name>
```

Conversations created after the snapshot are kept. With full sync on, a
restored conversation becomes the newest version, so it spreads back to every
account, and a restored deleted conversation is un-deleted in every account.
The restore itself is backed up first, and it tells you how to undo it. You can take a
full snapshot of every account at any time with `meld backup`.

## About conflicts

Occasionally the same conversation exists in two accounts with different
content — usually because it was continued separately in each. By default
meld never guesses which version you want: both copies stay exactly as they
are, and the conflict is listed in `meld status`. With `propagate_updates`
on, the copy that was actually edited wins: meld remembers the version your
accounts last agreed on, so a copy that was only opened (Claude touches a chat
just by showing it) never overwrites a real edit. If two accounts were both
edited, or meld has no history for the chat yet and the modification times
tie, it stays a conflict. Likewise, a deletion never removes a copy that was
edited after the accounts last agreed; that chat is listed as a conflict. Nothing about a conflict blocks the rest
of your history from syncing.

## Your safety, guaranteed by design

- By default meld never deletes or overwrites a file. With full sync on, it
  only deletes or replaces a conversation to mirror what you did in another
  account, and every such file is backed up first.
- A file that changed since meld looked at it is skipped, never clobbered.
- Every change can be undone with one command (`meld restore`).
- Files are written atomically, so an interruption (crash, power loss) can
  never leave a half-written conversation behind.
- Everything is local. Meld makes no network connections.

## Where meld keeps its own files

Everything meld owns lives in one folder: `~/.meld` (settings, its index,
and snapshots). Your conversations are never moved from their normal
location. Removing `~/.meld` resets meld completely without touching any of
your chat history.

## Settings (optional)

Meld works with zero configuration. If you want to change defaults — for
example, keep more snapshots — create a settings file and edit it:

```bash
meld config init     # creates ~/.meld/config.toml
meld config show     # view current settings
```

## All commands

| Command | What it does |
|---|---|
| `meld sync` | Make every account hold the same conversations |
| `meld sync --dry-run` | Preview a sync without changing anything |
| `meld watch` | Sync automatically whenever something changes |
| `meld status` | Show accounts, counts, pending work, conflicts |
| `meld accounts` | List every account and organization meld found |
| `meld diff` | List exactly what a sync would copy |
| `meld backup` | Save a snapshot now |
| `meld restore` | Roll back to the newest snapshot (`--list` to see all) |
| `meld service install` | Sync in the background at every login (macOS) |
| `meld service uninstall` | Turn background sync off |
| `meld doctor` | Verify folders and permissions are healthy |
| `meld scan` | Rebuild meld's index of your sessions |
| `meld config init` | Create the optional settings file |

## Uninstall

```bash
meld service uninstall     # if background sync is on
rm /usr/local/bin/meld     # or: brew uninstall meld
rm -rf ~/.meld             # optional: remove settings and snapshots
```

Your conversations are unaffected either way.
