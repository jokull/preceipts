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

## The runtime tier

Every service above is a `native` process — isolated by port and hostname,
costing nothing, which is the default and the reason parallel worktrees are
cheap. There is no hypervisor in this project and there will not be one: the
direction doc is emphatic that a mini-OrbStack is a VMM *plus* OCI images,
filesystem sharing, networking and lifecycle management, and is the least
differentiated code in the product.

What there is instead is an adapter. `image = "…"` implies `runtime =
"container"` and delegates to whatever is installed — Apple `container`,
OrbStack, Docker, colima. The last stanza of `preceipts.toml` is a commented
`icebox` service that does exactly that, verified against OrbStack: the daemon
publishes the container's port on the one it allocated, probes it, and reports
it alongside the node services with the same shape of address.

That sameness is the whole claim. The workspace, the URLs, the instruments and
the receipts do not change when a microVM is behind the address instead of a
process. Uncomment it if you have a runtime; leave it if you do not, and the
stand still opens.

## The transcript needs the CA

`preceipts requests` stays empty until the local CA is installed, and so does
the lab panel's `requests` instrument. Without it there is no TLS proxy, the
`*.localhost` names have nothing listening behind them, and the addresses you
get are the allocated ports — which work, and which the proxy never sees.

```sh
preceipts trust install     # prompts for sudo; Touch ID works
preceipts down && preceipts up
```

After that the hostnames are the addresses, the counter page's label-swapping
trick has something to swap *to*, and every request either service serves shows
up in the transcript with its status and duration.
