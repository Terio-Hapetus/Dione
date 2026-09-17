# Dione Roadmap

> Chi tiết M3→M15 (agent-agnostic + Workspace/Container). Spec: `ARCHITECTURE-v2.md`,
> `WORKSPACE-VM.md`, `AGENT-ANY.md`. Cách làm: `WORKFLOW.md`. Labs: `LABS.md`.

## M0 — Bootstrap ✅ done (`7d8f4e1`)

Workspace, AGENTS.md, `base` worktree/state pure logic, `desktop`
window skeleton opening under Xvfb.

## M1 — Single-agent ✅ done (`5bd2a94`, `66ec918`)

`opencode serve` spawn + client, Store mirror, SSE pump + poll reconcile,
context view-model, full UI (sidebar/timeline/composer/right
panel/permission gate/model picker). 15 tests, clippy 0 warnings.

## M2 — Fleet multi-agent ✅ done (`7d420e7`)

One task = one isolated git worktree, N agents in parallel.

- [x] `worktree.rs`: real git ops — create (`git worktree add`), list,
      remove, prune stale, `.worktreeinclude` copy (`495e044`)
- [x] 1 opencode session per worktree (session ↔ worktree link in Store,
      per-directory clients + SSE pumps) (`90c8a09`)
- [x] Dashboard: `Needs you / Working / Done` in grouped Fleet sidebar
- [x] Fan-out: 1 prompt → N worktrees (`⇉ all`); grouped multi-session
      diff compare (`↻ all`)
- [x] Annotate diff lines → send batch back to the right agent
- [x] Merge winner (`--no-ff`) + prune; keep dirty worktrees for manual
      recovery
- [x] Cap ~15 managed worktrees (enforced in `worktree::create`)

Conventions: path `<repo>/.dione-worktrees/<slug>`, branch `dione/<slug>`;
one branch in one worktree; detached HEAD for experiments.
Gitignore `.dione-worktrees/` in every target repo (else `git add .` warns
about embedded repos).

## M3 — Agent-agnostic nền (transcript + trait + git-diff)

UI hết import opencode; diff qua git để mọi agent dùng được.

- [x] `transcript.rs`: `UnifiedMessage`/`Cost`, Store dual-write + test mirror
- [x] `agent.rs`: `AgentBackend` trait + `OpencodeAdapter` (wrap `server.rs`)
- [x] `git_diff` thay `GET /session/{id}/diff` (`worktree.rs` + test)
- [x] UI Chat đọc `transcripts` (mỗi slice `app.rs` <200 dòng)

## M4 — Workspace + Task (Host trước, chưa VM)

- [x] `workspace.rs` + `HostProvider` + `Task { id, slug, agent_ref }`
- [x] `agents.toml` + probe `which <bin>` (tick xanh/đỏ ở Agent picker)
- [x] `Supervisor` (backend+workspace/task) + kanban-lite `Dispatcher` 60s
      (structs + tests; runtime sweep wiring pending)

## M5 — Sandbox container (1 container / workspace, ADR-0006 thay MicroVM)

- [x] `PodmanProvider` (exec `-e` secrets-via-env, shell `exec -it` dưới pty)
      + `probe_podman` → fallback Host (CI xanh không podman)
- [x] `ContainerManager` (probe→pull→run→ready, timeouts) + `ContainerState`
      (`Missing|Pulling|Running|Stopped|Error`) + image pin + live-gate
      `DIONE_LIVE_PODMAN=1`
- [x] UI: badge `ctr:` ở Fleet + banner Host mode khi mất podman (Lab 6)
- [x] Xóa `vm` (CH/seed/vsock/uds/keys/manager) + `microvm.rs` + `ssh_info`
- [ ] Live lần đầu trên máy podman: Lab 2/3 xanh
- Ghi chú: MicroVM (CH) đã implement để validate seam rồi xóa (ADR-0006),
  podman rootless là engine duy nhất. History còn trong git.

## M6 — Terminal modern (Warp-like + container attach)

- [x] Local pty tab + scrollback/search (Host: `ShellChannel`/`HostShell`,
      terminal tab viewer + input + filter; Xvfb smoke sạch)
- [x] `PodmanProvider::shell` (`exec -it` dưới pty, preview `-p 41xx:3000`)
      + `ContainerThread` (Ensure/Stop/OpenShell, seam + fake-podman tests)
- [x] `TerminalAdapter` (`portable-pty`) + kit đầu `kits/opencode.sh`
      (pin/verify + fake-guest tests; `Working` heuristic + `mark_done`)
- [ ] Còn lại: Open-Workspace container wiring production (`rt.fleet()`
      callsite: `open_task_with` với `PodmanProvider`) + live podman

## M7 — Fleet reliability (học Hermes kanban) ✅ done (Host-only)

- [x] Retry budget + circuit breaker (`failure_limit=2 → blocked`, per-task)
- [x] Structured handoff `summary + parent link` (data-only UI)
- [x] Worktree hygiene: base `origin/HEAD` (offline-safe), 1 subtask = 1 worktree riêng

## M8 — Review++ (học Orca aggregator + Codex review queue) ✅ done (trên branch M7)

- [x] Review queue sort chờ-lâu-nhất (NeedsYou + updated cũ nhất, fair queue);
      split-view 2 cột để sau; cherry-pick hunk (`split/apply` + Diff tab picks)
- [x] `Hand off to local` (merge giữ worktree) vs `Create branch here`;
      annotate multi-line (two-click range) + thread replies
- [x] Editor-lite p1: read-only viewer text-first (tab File, gutter số dòng,
      cap 256KB/2000 dòng); Tree-sitter grammars để sau (cần mạng)

## M9 — Cost / BYOK ✅ done (`243eafb`)

- [x] `metrics.rs` (`a185480` → `243eafb` M9a: `UsageSample{cost: Option}` + `MetricsLog` record/aggregate/window/group-by + `from_cost` bridge, `Cost` cũ frozen)
- [x] `usage_probe.rs` (M9b: parse Claude `message.usage` + Codex `token_count` JSONL, defensive skip, `scan_jsonl_logs`, `QuotaWindow` + `samples_for`)
- [x] `AgentEvent::Usage` + `Supervisor.metrics` (M9c: stamp agent/model từ `Task`, opencode bridge edge-triggered, `MockAgent`/applier arms)
- [x] Control-room view (M9d: `SupervisedTask::usage_samples` + `FleetInbox::collect_usage` + `DioneApp::usage` poll → tab `Costs` per-agent/model, 5h tokens, `~`/`n/a` money)
- [x] Keychain BYOK (M9e: `Secrets` trait + `MockSecrets` + `CliSecrets` via `secret-tool`, `AgentEntry.env_keys` + `resolve_task_env`, `exec_with_env` Host/Podman, ticks `◆/◇`)
- [x] Rate-limit visibility (M9f: `Retry` → NeedsInput/NeedsYou + `TerminalAdapter` markers pty, latch tới Done; `worktree_status` + review queue)
- Ghi chú: `Cost` cũ frozen, account switcher để sau (→ M10). Keychain live-gate máy GNOME (CI vẫn xanh).

## M10 — Remote / Notify + Account switcher (học Orca SSH + mobile)

- [ ] Account switcher (BYOK profiles, đổi env theo agent — tách từ M9)
- [ ] SSH remote worktrees + host picker `~/.ssh/config`; cross-host dashboard
- [ ] Telegram ping `done/needs-input` + `/followup` (trước app mobile)

## M11 — Factory as-code (học Warp Factories)

- [ ] `factory.yaml` (triggers/agents/gates) + settings sync
- [ ] Automations cron/webhook; control-room runs

## M12 — Evals / self-improve

- [ ] Scorers mặc định + custom hook; Benchmarks so 2 configs
- [ ] Memory per-repo (đề xuất update AGENTS.md). Không guardrails engine.

## M13 — Task integrations

- [ ] GitHub/Linear issues/PRs/boards in-app; `Open worktree from task`
- [ ] Remote HTML preview

## M14 — Hardening + Packaging

- [ ] Defense-in-depth (hardening flags ở WORKSPACE-VM.md); audit secrets
- [ ] AppImage/deb + size opt; podman-less + offline fallback tests
- [ ] P1 KHÔNG: Docker-in-container, GPU passthrough, Balanced/Locked enforce

## M15 — 1.0 polish

- [ ] Perf 120fps audit; docs+labs full; dogfood `factory.yaml` cho repo này

## Out of scope (giữ nguyên)

- Infinite canvas (separate project), LSP/debug/marketplace đầy đủ,
  model harness internals (memory routing, eval loops nặng).
