# ADE Roadmap

> Chi tiết M3→M15 (agent-agnostic + Workspace/MicroVM). Spec: `ARCHITECTURE-v2.md`,
> `WORKSPACE-VM.md`, `AGENT-ANY.md`. Cách làm: `WORKFLOW.md`. Labs: `LABS.md`.

## M0 — Bootstrap ✅ done (`7d8f4e1`)

Workspace, AGENTS.md, `ade-core` worktree/state pure logic, `ade-ui`
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

Conventions: path `<repo>/.ade-worktrees/<slug>`, branch `ade/<slug>`;
one branch in one worktree; detached HEAD for experiments.
Gitignore `.ade-worktrees/` in every target repo (else `git add .` warns
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

## M5 — MicroVM lõi (1 VM / workspace)

- [x] `vm.rs` + `Mock` + probe `/dev/kvm` → fallback Host (CI xanh không KVM)
- [x] `CloudHypervisorBackend`: boot → wait_ssh → mount virtiofs → stop
      (REST qua UDS, seed cloud-init, proxy vsock; live-gate `ADE_LIVE_VM=1`)
- [x] `VmManager` (probe→boot→ssh→mount→ready, timeouts) + `MicroVm`
      provider (exec ssh, secrets-via-env) + `EphemeralKey` + image pull/verify
- [x] UI: badge `VmState` ở Fleet + banner Host mode khi mất KVM (Lab 6)
- [x] Tách crates `ade-workspace` / `ade-vm` khi API ổn định
- [ ] Live lần đầu trên máy KVM: Lab 2/3 xanh, re-verify flag virtiofsd/vsock
- Ghi chú: `ExternalSbx` đã implement để validate sớm rồi xóa (ADR-0005),
  CH là backend duy nhất.

## M6 — Terminal modern (Warp-like + SSH attach)

- [ ] Local pty tab + scrollback/search
- [ ] SSH attach (key ephemeral) + port-forward preview `VM:3000 → host:41xx`
- [ ] `TerminalAdapter` (`portable-pty`) + kit đầu `kits/opencode.sh`

## M7 — Fleet reliability (học Hermes kanban)

- [ ] Retry budget + circuit breaker (`failure_limit=2 → blocked`)
- [ ] Structured handoff `summary/metadata + parent link`
- [ ] Worktree hygiene: base `origin/HEAD`, 1 subtask = 1 worktree riêng

## M8 — Review++ (học Orca aggregator + Codex review queue)

- [ ] Review queue sort theo chờ-lâu-nhất; split-view 2 cột + cherry-pick hunk
- [ ] `Hand off to local` vs `Create branch here`; annotate multi-line + thread
- [ ] Editor-lite p1: read-only viewer (Tree-sitter), mở file từ diff

## M9 — Cost / BYOK

- [ ] `metrics.rs`: tokens/cost per task/model/agent; control-room view
- [ ] Account switcher + rate-limit visibility; secrets qua env (không vào VM)

## M10 — Remote / Notify (học Orca SSH + mobile)

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

- [ ] Defense-in-depth (native sandbox trong VM); audit secrets
- [ ] AppImage/deb + size opt; KVM-less + offline fallback tests
- [ ] P1 KHÔNG: Docker-in-VM, GPU passthrough, Balanced/Locked enforce

## M15 — 1.0 polish

- [ ] Perf 120fps audit; docs+labs full; dogfood `factory.yaml` cho repo này

## Out of scope (giữ nguyên)

- Infinite canvas (separate project), LSP/debug/marketplace đầy đủ,
  model harness internals (memory routing, eval loops nặng).
