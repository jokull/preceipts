# preceipts

**Everyone is building the agent harness. Nobody is building the room it works in.**

You have agents writing code now, maybe three at once. Each one needs a
branch, an environment, and some way for you to find out what it actually
did. What you have instead is six worktrees you can't tell apart, a
`:3001` you can't remember the owner of, a dev server you've restarted
four times today, and a GitHub PR page you're scrolling at half a second
per file.

The writing got fast. Everything around the writing is still slow.

preceipts is a macOS app for the everything-around. It never writes code
and never runs your agent — you keep doing that, in your terminal, with
whatever you like.

## What it does

**One tab per project.** Type what you want done. You get a git worktree,
a branch named from what you typed, a dev environment booted and
healthchecked, and real URLs — `api.fix-checkout.trip.localhost`, not a port
you have to look up. The prompt sticks to the workspace, so a list of
eight worktrees is still readable next Tuesday.

**A diff that keeps up.** Virtualized, tree-sitter highlighted, patience
diff with word-level intraline. It updates while the agent types, because
it's watching the filesystem, not waiting for a commit.

**Checks fire when the typing stops.** The agent goes quiet, the checks
run in that workspace's already-warm environment, the tab turns green or
red. No push, no CI queue, no tab switch. Results are keyed to the git
**tree hash**, so they survive rebase, reword, and squash — if the content
is identical, the proof still stands.

**Your environment is a laboratory, and your agent knows it's inside one.**

```bash
preceipts proc api tail -n 200          # logs, cursor-addressable
preceipts proc api signal HUP --wait 5s # restart, block until healthy again
preceipts services                      # what is up, healthy, and on which URL
preceipts status                        # the receipt table for this tree
```

Everything the app shows you, an agent can query — same socket, same
truth, no UI-only features. There's an MCP server, so any harness that
speaks MCP picks it up natively — no plugin, no vendor. A worktree
identifies itself through the environment, so any agent started in that
directory finds its lab without being configured.

The HTTP transcript is the one to notice, and it is the next thing being
built rather than a thing that works today. The local TLS proxy is already
in the path of every request into every service, which is why recording
them costs nothing to instrument: your agent will be able to read exactly
what its code served and answered, with nothing added to your app.

## What it isn't

Not an IDE. Not an agent harness — there's no terminal pane and no chat
window, by design. Not a CI service; nothing runs on a server, and there's
no queue. Not a GitHub client.

It's the other window. The one open beside your agent, showing you what's
happening and telling you whether it's safe to land.

## How it works

macOS-native where it counts: secrets in the Keychain, a local CA so
`https://` just works, FSEvents for the watching, and `*.localhost`
hostnames the system resolves on its own — no DNS to install and none to
leave behind. Signing brings the rest: a shared Keychain access group,
Touch ID to reveal a secret, and the daemon registered so your dev servers
outlive the window.

One Rust workspace: an in-process core for git, diff, and highlighting (no
IPC on the scroll path), a daemon for processes, health, and proxying, and
a CLI that's a first-class client rather than an afterthought.

Local-first and offline. Your code, your machine, your keychain.

## Who it's for

Developers running coding agents against real applications — the kind
with a database, a few services, and an actual browser surface — who are
tired of "it worked on my machine" being the only evidence an agent can
produce.
