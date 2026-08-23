# The Hot Dog Stand

A sandbox to point preceipts at.

preceipts does not need it. It exists because the lab panel, the URL fabric and
the HTTP transcript are hard to judge against a repository that has no services
in it — and because a demo whose services are called `api` and `web` teaches
you nothing about what an address is *for*. This one has a grill, a condiment
supplier and a counter, and every one of them maps onto something real.

## Run it

```sh
cd examples/hot-dog-stand
preceipts up
preceipts services      # what is running, and on what URL
open "$(preceipts services --json | jq -r '.[] | select(.name|test("web")) | .url')"
preceipts requests      # every exchange the proxy carried
preceipts down
```

Nothing to install. No `package.json`, no lockfile, no build. Four `run =`
lines over plain `node`, which is the smallest shape a project can have and
still be a project.

## What each service is there to prove

| Service | Address | What it demonstrates |
|---|---|---|
| `grill` | none | A one-shot with no health block. Everything `needs` it; nothing starts until it exits 0. |
| `condiments` | `condiments.…localhost` | A service is a thing with an address and a healthcheck. Where it runs is not the address's business. |
| `api` | `api.…localhost` | `$PORT` is allocated and handed over. The service never picks its own number — which is why two worktrees of this project can run at once and neither has to be told. |
| `web` | `web.…localhost` | The page finds its neighbours by *name*: it swaps the first label of whatever host it was reached on. It works unchanged in a linked worktree, where every name has a workspace segment in the middle. |

That last one is the whole point of the fabric. `web.stand.localhost` becomes
`api.stand.localhost`; in a worktree it becomes
`api.fix-the-mustard.stand.localhost`; the page's code does not change and was
never configured.

## What it does not do

No micro-VM. The direction doc is emphatic about not writing a hypervisor, and
this project would be the wrong place to prove otherwise — every service here
is a `native` process, isolated by port and hostname, which is the default
runtime and the one that costs nothing.

If you want the containerised tier, it is one line per service — `image =
"…"` implies `runtime = "container"` — and it delegates to whatever is
installed (Apple `container`, OrbStack, Docker). Adding it here would mean
this example stopped running for anyone with none of them, in exchange for
demonstrating an adapter rather than a fabric.
