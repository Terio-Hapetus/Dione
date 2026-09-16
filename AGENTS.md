# Dione — Agentic IDE

Native Linux desktop app (Rust + GPUI) to run and inspect AI coding agents.
One task = one isolated git worktree, many agents in parallel, inline diff
review with notes sent back to the agent. Agent engine is `opencode serve`,
driven via the Rust SDK `opencode-codes`.

No canvas — that lives in a separate project. Focus: M2 fleet (see
`docs/ROADMAP.md`).

> New session? Read `docs/STATUS.md` first (current position), then
> `docs/ARCHITECTURE.md`. Load skills on demand via the `skill` tool.

## Stack

- Rust 2024 edition, MSRV `rust-version = "1.90"`, workspace resolver 3.
- `gpui = "=0.2.2"` from crates.io only — never mix with zed git deps
  (type conflicts). Linux renders via blade/Vulkan.
- `gpui-component = "=0.5.1"` + `gpui-component-assets`.
- `opencode-codes 1.18` (`async-client`, `server`), tokio multi-thread,
  reqwest `rustls-tls` with `default-features = false` (avoid dual TLS).
- System deps: build-essential, libxkbcommon(-x11), libwayland, X11 client
  libs, x11-utils, xvfb, mesa-vulkan-drivers, DejaVu fonts.
- Details: `docs/GPUI.md`, `docs/ARCHITECTURE.md`.

## Commands

```bash
cargo check --workspace
cargo clippy --workspace --all-targets   # 0 warnings
cargo fmt --all                          # before every commit
cargo test -p base               # unit, no network

# Live (spawns real `opencode serve`):
cargo test -p base --features integration-tests
DIONE_LIVE_PROMPT=1 cargo test -p base --features integration-tests tier_b

cargo run -p desktop
# Headless smoke (unset Wayland or GPUI picks it over Xvfb):
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET xvfb-run -a -s "-screen 0 1440x900x24" ./target/debug/dione
```

## Docs map

- `docs/VISION.md` — what Dione is / is not, success criteria
- `docs/ARCHITECTURE.md` — crates, Store, Commands, runtime loop, UI
- `docs/ROADMAP.md` — M0/M1 done, M2 tasks, out of scope
- `docs/STATUS.md` — living file: current position, next up, blockers
- `docs/RESEARCH.md` — Orca/Codex/Hermes/Warp/tldraw takeaways
- `docs/GPUI.md` — ecosystem + API pitfalls

## Key pitfalls (details in docs/GPUI.md + skills)

- `overflow_y_scroll()`/`.on_click()` only on `.id(...)` elements;
  `rgba(u32)` is NOT const — wrap in helper fns.
- `WeakEntity::read_with(cx, |app, cx_app| ...)` takes 2 args.
- SSE is best-effort — always reconcile via REST (`list_messages`) on
  interval and after `StreamEvent::Connected`; wildcard arm required.
- Wire: `SessionStatus` is `{"type":"busy"}`; envelope uses `properties`;
  `portpicker` is NOT re-exported by the SDK.

## Workflow

- Plan mode for design, build mode for code. Small slices (`feat(mX):`,
  <300 lines). `cargo fmt` + clippy + test before every commit.
- End of every task: update `docs/STATUS.md` (position/next/blockers).
- Solo learn-build loop: research 30% → slice <3 days → `cargo test` + demo.
