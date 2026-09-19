# AGENT-ANY Spec (agent-agnostic: không khóa agent mặc định)

## Nguyên tắc

- Dione mặc định là **Terminal + Chat GUI**. Agent là backend cắm vào.
- Không bundle model. BYO subscription/key. Phát hiện binary, user chọn per-task.
- Chat và Terminal cùng cwd = worktree. Agent không biết mình ở Host hay container.

## Thêm agent mới trong 3 bước

1. Thêm kit script `kits/<ten>.sh` (cài binary lúc container boot, không bake vào image; hiện chỉ có `kits/opencode.sh`, `claude.sh`/`codex.sh` trong `agents.toml.example` vẫn là mẫu comment).
2. Thêm 1 dòng vào `agents.toml`:
   ```toml
   [agents.<ten>] bin = "<bin>" prompt_arg = "-p"
   ```
3. Chạy lab `LABS.md` Lab 4 (probe → spawn → prompt → diff hiện).

Xong: agent xuất hiện trong Agent picker, chạy được fan-out so với agent khác.

## Hai adapter có sẵn

| Adapter | Khi nào dùng | Transcript | Status |
|---|---|---|---|
| `OpencodeAdapter` | Agent có API/SSE (opencode hôm nay) | Rich (SSE) | Chính xác |
| `TerminalAdapter` | Mọi CLI (Claude Code, Codex, Gemini…) | pty scrollback | Heuristic + nút `Mark done` + latch rate-limit → NeedsInput tới Done |

- `TerminalAdapter` dùng `portable-pty`. Hiển thị `Working(?)` khi không chắc.
  Rate-limit (`rate limit`/`429`/`quota exceeded`/`overloaded`) latch `NeedsInput` tới khi `Done` (M9f); opencode `Retry` cũng `NeedsInput`.
- Permission: adapter không support → hướng dẫn user trả lời trong terminal.

## Secrets (M9e BYOK)

`agents.toml` cho phép `env_keys = ["ANTHROPIC_API_KEY", ...]` per agent.
Keys sống ở OS keychain (`secret-tool` → Secret Service, `account = "<agent>/<KEY>"`), bơm qua `-e K=V` lúc spawn (Host và `podman exec`), không vào container FS/log/UI. Thiếu key → tick `◇` (đủ → `◆`, values không bao giờ hiện). Không `secret-tool` → fallback env, CI vẫn xanh.

## Per-task override (học Hermes)

`Task { id, slug, agent_ref, model_override, max_runtime_secs }`
(+ `failure_limit/failure_count/status`, `parent/summary` handoff).
Fan-out 1 prompt → N tasks khác agent → compare diff git → merge winner.

## Không làm

- Không LSP/editing nặng. Editor-lite chỉ để review (đọc + sửa lặt vặt).
- Không guardrails engine. Permission gate hiện tại giữ nguyên.
