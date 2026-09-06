# ADE Status (living file — update at the end of every task)

## Current milestone

M4 — Workspace + Task (Host) started. Restructure DONE (5 crates).

## Last commit (đã verify)

- `3a7091d` refactor(core): `state.rs` → `state/{types,store,events}` + facade
  (pure move, public paths giữ nguyên) — 36/36 xanh
- `70aac9c` refactor(core): `runtime.rs` → `runtime/{commands,session,sse,
  reconcile,handlers,io}` + 3 tests pure mới (drop_scope/scoped_sessions/
  sessions_for_scope) — 39/39 xanh
- `cb2cc0c` refactor(ui): `app.rs` (1145) → `app.rs` (~200) +
  `views/{theme,top_bar,sidebar,chat,composer,right_panel,diff,
  permission}` + 2 refactors nhỏ (xóa `selected_part` chết, xóa diff
  note theo value thay vì index) — clippy sạch
- `4bf4d0e` feat(m5a): crate `ade-vm` (VmConfig/NetPolicy/SshInfo/VmHandle/
  VmState + VmBackend trait + MockBackend + probe_kvm) — 4 tests xanh
- `ad83820` refactor(core): move `agent.rs` → crate `ade-agent` (one-way
  dep vào ade-core; core 39→34, agent 5 tests đi theo)
- `82e7042` feat(m4a): crate `ade-workspace` (Task + WorkspaceProvider seam
  + HostProvider + MockWorkspace) — 6 tests xanh
- Verified: `cargo check --workspace` + clippy 0 warnings (ngoài
  future-incompat của dep `proc-macro-error2`) + `cargo fmt` sạch +
  `cargo test --workspace` 49 passed (14 core unit + 5 agent + 4 vm +
  6 workspace + 13 state + 7 worktree), Xvfb smoke clean (app sống,
  không panic, stderr rỗng).
- File lớn nhất còn lại: `runtime/handlers.rs` 293 dòng (~250 code +
  tests), `state/store.rs` 348 (code ~270 + tests) — chấp nhận được,
  tách tiếp khi M4 runtime supervisors đụng vào.

## Next up

1. M4 tiếp: `agents.toml` + probe `which <bin>` (tick xanh/đỏ ở Agent picker).
2. M4 tiếp: runtime supervisors thay clients + kanban-lite dispatcher 60s.
3. M5 tiếp: `CloudHypervisorBackend` / `ExternalSbxBackend` cho `ade-vm`.
4. Tier B live prompt: opt-in `ADE_LIVE_PROMPT=1 cargo test -p ade-core
   --features integration-tests tier_b` — tốn quota, chạy tay khi cần.

## Blockers

- None. Tier B cần user duyệt chi quota (keys đã có sẵn trong env).
