# Dione Architecture v2 (agent-agnostic + Workspace/Container)

> ADR-0006: MicroVM → podman. "VM/guest/SSH/virtiofs" còn sót trong file
> này nghĩa là container tương ứng (1 container / workspace, bind-mount).

> V1 (`ARCHITECTURE.md`) mô tả M1–M2 opencode-coupled. V2 là mục tiêu
> M3→M15. V1 giữ nguyên để tra cứu legacy; code mới bám V2.

## Sơ đồ lớn

```
┌─ Host (mặc định, Warp-like) ─────────────────────────┐
│ Dione UI (GPUI) + local terminal (pty) + SSH client    │
│ secrets ở keychain host, chưa chạy agent ở đây       │
└───────────────────────┬──────────────────────────────┘
                        │ chỉ khi Open Workspace / Run Agent
┌─ Workspace (1 repo) ──▼──────────────────────────────┐
│ 1 container / 1 workspace (podman rootless)          │
│ ├── worktrees trong bind-mount /workspace            │
│ │    <repo>/.dione-worktrees/<slug> (branch dione/<slug>)│
│ └── agent CLI bất kỳ (kit script lúc boot)           │
└──────────────────────────────────────────────────────┘
```

- Source of truth = **repo trên host**, mount read-write vào container
  qua bind-mount. Merge trong container hiện ngay trên host.
- Không `podman` → fallback Host mode + banner, không crash.

## Crates (mục tiêu, 1 chiều, không vòng)

```
crates/base/        # FROZEN M1–M2: Store/Command cũ giữ nguyên (dual-write)
                    # + transcript/metrics/runtime-fleet seams (agent←workspace←base)
crates/workspace/   # Task + WorkspaceProvider { Host, Podman } + memory.rs
                    # + ContainerManager (ensure/stop/pause, image pull) — ADR-0006/0007
crates/agent/       # AgentBackend { OpencodeAdapter, TerminalAdapter } + kits/opencode.sh
apps/desktop/       # binary `dione`: HostShell + WorktreeView [Chat | Terminal]
```

Quy tắc chống vỡ legacy:

1. Không sửa signature `Store`/`Command` cũ — chỉ **thêm** types mới.
2. Module mới sống trong `base` ở M3–M4 (<250 dòng/file), tách crate
   khi API ổn định (M5).
3. CI/Xvfb không podman vẫn xanh (fakes + live-gate `DIONE_LIVE_PODMAN=1`).
4. Build mặc định `host-only` không cần podman.

## Core types (khóa để review)

```rust
// transcript: biên bản chung cho mọi agent
enum Role { User, Agent, Tool }
struct UnifiedMessage { id, task: TaskId, role: Role, text, tool: Option<ToolCall>, ts: u64 }
struct Cost { input, output, cache, cost: f64 } // legacy, frozen M9a
// M9 costs: Cost frozen, UsageSample{cost: Option<f64>} (None = tokens-only)
// + MetricsLog (record/aggregate/window, cost_unknown) + usage_probe.rs
// (Claude/Codex JSONL) → AgentEvent::Usage → Supervisor.metrics (stamp
// agent/model từ Task, Store frozen) → FleetInbox::collect_usage →
// DioneApp::usage → tab Costs (per-agent/model, 5h tokens, ~ / n/a).
// M12 memory (orchestrator, suggest-only): RepoMemory cap 50 FIFO +
// MemoryEntry{ task, slug, agent_ref, kind: Win/Fail/Note, text≤140, ts }
// pure: distill_entry(&Task, kind, ts) -> Option<MemoryEntry> (dòng đầu
// summary|slug sanitize, capped đúng 140…); propose_agents_patch(
// &RepoMemory, take) -> Option<String> (snippet AGENTS.md với sentinels)
// + merge_into_agents_md (idempotent Apply); recall_context(_capped)(
// &RepoMemory, take[, max]) -> Option<String> (context block cho child/
// retry qua driver child_with_memory); MemoryStore per-repo owner;
// Supervisor::distill helper. Auto-drain khi Blocked → store: backlog.

// agent: ổ cắm thay được, không khóa opencode
trait AgentBackend: Send {
    fn spawn(&mut self, task: TaskId, prompt: &str) -> Result<()>;
    fn prompt(&mut self, task: &TaskId, text: &str) -> Result<()>;
    fn abort(&mut self, task: &TaskId) -> Result<()>;
    fn poll(&mut self) -> Vec<AgentEvent>;
    fn collect(&mut self, store: &Store) -> Vec<AgentEvent>; // OpencodeAdapter: cursor dedup
    fn bind_session(&mut self, task: TaskId, session_id: &str);
    fn unbind_session(&mut self, session_id: &str);
}
enum AgentStatus { Idle, Working, NeedsInput{ reason: String }, Done, Error{ msg: String } }

// workspace: Host và container chung 1 mặt
trait WorkspaceProvider: Send {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> Result<ExecOut>;
    fn exec_with_env(&mut self, cmd: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Result<ExecOut>;
    fn shell(&mut self, cwd: &Path) -> Result<Box<dyn ShellChannel>>;
}

// container: 1 container / 1 worktree, lazy wake/pause (ADR-0007)
enum ContainerState { Missing, Pulling, Running, Paused, Stopped, Error(String) }
struct ContainerManager { /* ensure_running / stop / pause / unpause over the podman CLI */ }
```

- `OpencodeAdapter` bọc nguyên `server.rs` + SSE/poll hiện tại.
- `TerminalAdapter` dùng `portable-pty`, status heuristic + nút `Mark done`.
- Agent không biết mình ở Host hay container (chỉ thấy `WorkspaceProvider`).

## Luồng chính

**Boot workspace:** `Open` → probe `podman` → pull image (nếu thiếu) →
run → Running → mở Terminal tab.
**Chạy agent:** `Run` → `ContainerManager` đảm bảo Running → kit script
cài agent (nếu thiếu) → `AgentBackend::spawn(ws, prompt)` → poll
`AgentEvent` → dịch về `UnifiedMessage` → Chat render.
**Review:** `git_diff` qua mount → compare/cherry-pick → merge winner
(`--no-ff`) → prune. Secrets bơm qua env lúc exec, không ghi đĩa container.
**Rớt mạng/podman:** `Error` + banner, giữ worktree để retry tay.

## Dữ liệu (Store dual-write 2 milestone)

`Store` cũ (`sessions: Session`, `messages: Message/Part`) giữ nguyên;
thêm `transcripts: Map<TaskId, Vec<UnifiedMessage>>` + `Cost`.
UI mới chỉ đọc `transcripts`. `apply_event(&Event)` thu hẹp thành
`apply_agent_event(&AgentEvent)` ở adapter. Xóa types opencode khỏi
`app.rs`/`context.rs` ở M3d (sau khi mirror đủ).
