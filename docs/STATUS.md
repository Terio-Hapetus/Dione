# ADE Status (living file — update at the end of every task)

## Current milestone

M3 — Agent-agnostic nền DONE. Next: M4 Workspace + Task (Host).

## Last commit (đã verify)

- `47cba78` feat(m3a): `transcript.rs` (TaskId UUID + UnifiedMessage/Cost/
  entry_to_unified + Store dual-write + mirror test) — check/clippy/fmt
  xanh, 30/30 unit (26 cũ + 4 mới), workspace check pass
- `4dfdeb0` feat(m3b): `agent.rs` (AgentBackend + OpencodeAdapter/collect_new/
  apply_agent_event + Mock, 5 tests) — 35/35 unit, clippy/fmt xanh
- `700c760` feat(m3c): `git_diff` (GitDiff/files+raw/to_json + fetch_diff
  git-first + HTTP fallback + worktree tạm test) — 36/36, clippy/fmt xanh
- `89a94a5`/`2c8f24c`/`a5d715a` feat(m3d 3 slices): Chat v2 đọc transcripts →
  xóa Timeline cũ (app.rs hết import opencode) → context.rs đọc costs —
  Xvfb smoke clean ×3, 36/36 unit
- `bed2d72` feat(m2f): Tier A live test (`tests/live_tier_a.rs`, 196 dòng)
  — serve thật + 2 worktrees + sessions + diff fetch + merge winner +
  remove, 2 passed in ~9s, không tốn prompt
- `41de1a5` docs: v2 plan M3–M15 + specs + ADRs + labs (13 files)
- Verified: check pass, clippy 0 warnings (ngoài future-incompat của
  dep `proc-macro-error2`), 26/26 unit + Tier A xanh, `cargo fmt` sạch,
  Xvfb smoke clean (app sống 60s, không panic).
- Local `master` → `main` (track `origin/main`); dọn worktree `task-1` thừa.
- Chưa push: local ahead `origin/main` 2 commits (`41de1a5`, `bed2d72`).

## Next up

1. M4 kickoff: `workspace.rs` + `HostProvider` + `Task` (theo ARCHITECTURE-v2).
2. Tier B live prompt: opt-in `ADE_LIVE_PROMPT=1 cargo test -p ade-core
   --features integration-tests tier_b` — tốn quota, chạy tay khi cần.

## Blockers

- None. Tier B cần user duyệt chi quota (keys đã có sẵn trong env).
