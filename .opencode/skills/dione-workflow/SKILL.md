---
name: dione-workflow
description: How to work on ADE — plan/build modes, verify gates, commit slices, STATUS updates
---

# ADE workflow

Load this skill when starting or finishing any ADE task.

## Modes

- Plan mode: design only, read-only, no file writes.
- Build mode: implement in small slices (`feat(mX):` prefix, <300 lines).

## Verify gate (before every commit)

```bash
cargo check --workspace
cargo clippy --workspace --all-targets   # must be 0 warnings
cargo fmt --all
cargo test -p base
```

GUI change? Also run the Xvfb smoke and confirm the window appears:

```bash
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET xvfb-run -a -s "-screen 0 1440x900x24" ./target/debug/dione
```

## Commit discipline

- Commit when a slice is done (`feat(m0): …`, `feat(m1a): …`, `docs: …`).
- Never commit secrets; stage only intended files (`git status` first).
- Solo loop: research 30% → slice <3 days → test + demo.

## End of every task

Update `docs/STATUS.md`: current milestone, last commit, next up, blockers.
