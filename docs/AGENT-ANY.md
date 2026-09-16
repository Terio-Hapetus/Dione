# AGENT-ANY Spec (agent-agnostic: không khóa agent mặc định)

## Nguyên tắc

- Dione mặc định là **Terminal + Chat GUI**. Agent là backend cắm vào.
- Không bundle model. BYO subscription/key. Phát hiện binary, user chọn per-task.
- Chat và Terminal cùng cwd = worktree. Agent không biết mình ở Host hay container.

## Thêm agent mới trong 3 bước

1. Thêm kit script `kits/<ten>.sh` (cài binary lúc container boot, không bake vào image).
2. Thêm 1 dòng vào `agents.toml`:
   ```toml
   [agents.<ten>] bin = "<bin>" prompt_arg = "-p"
   ```
3. Chạy lab `LABS.md#agent-mới` (probe → spawn → prompt → diff hiện).

Xong: agent xuất hiện trong Agent picker, chạy được fan-out so với agent khác.

## Hai adapter có sẵn

| Adapter | Khi nào dùng | Transcript | Status |
|---|---|---|---|
| `OpencodeAdapter` | Agent có API/SSE (opencode hôm nay) | Rich (SSE) | Chính xác |
| `TerminalAdapter` | Mọi CLI (Claude Code, Codex, Gemini…) | pty scrollback | Heuristic + nút `Mark done` |

- `TerminalAdapter` dùng `portable-pty`. Hiển thị `Working(?)` khi không chắc.
- Permission: adapter không support → hướng dẫn user trả lời trong terminal.

## Per-task override (học Hermes)

`Task { id, slug, agent_ref, model_override, max_runtime }`.
Fan-out 1 prompt → N tasks khác agent → compare diff git → merge winner.

## Không làm

- Không LSP/editing nặng. Editor-lite chỉ để review (đọc + sửa lặt vặt).
- Không guardrails engine. Permission gate hiện tại giữ nguyên.
