# preceipts

Local CI, proven by trees, merged by humans. Checks run on your machine (where
the environment already exists), results travel through the repo itself as
**receipts** — git notes keyed to tree hashes — and merging stays a human (or
agent) decision informed by evidence, never blocked by it. Full rationale and
design: [PRD.md](./PRD.md).

## Install

Requires [Bun](https://bun.sh) and git.

```sh
bun install
bun run build        # → dist/preceipts, a single self-contained binary
```

## Quickstart

```sh
preceipts init                       # scaffold .preceipts/, offer receipt refspecs on origin
cat > .preceipts/checks/test <<'EOF'
#!/bin/bash
bun test
EOF
chmod +x .preceipts/checks/test      # a check is any executable file; exit 0 = pass
# list it under [required] checks in .preceipts/config.toml, then:

preceipts run                        # run all checks in parallel, mint receipts
preceipts status                     # receipt table for HEAD's tree; exit 0 iff green
preceipts log test                   # stored output for a check
preceipts land feature --onto main   # verify receipts, squash, fast-forward, push
preceipts sync                       # exchange receipts with origin (lossless merge)
preceipts gc                         # prune old log refs (success 30d, failure 90d)
```

## The tree-hash idea

A receipt attaches to a **git tree hash**, not a commit. Rebases and message
edits don't invalidate work (same tree, same receipts); a local squash via
`git commit-tree` produces a commit whose tree *is* the branch tree, so the
receipts provably describe the merged result; and a dirty worktree gets a real
key — checks are minted against the exact state that was tested, becoming valid
the moment you commit that state. Because `.preceipts/checks/*` is committed,
the tree hash covers the check definitions for free: edit a check and prior
receipts stop matching automatically.
