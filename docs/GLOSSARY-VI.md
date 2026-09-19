# Từ vựng Dione (tiếng Việt, ví dụ đời thường)

Dành cho người không phải kỹ sư phần mềm. Đọc 1 lần, tra cứu khi gặp từ lạ.

## Host — máy bạn đang ngồi

Máy vật lý chạy Dione UI. Giống như "nhà chính". Mặc định app chạy ở đây
(terminal, SSH client, secrets). An toàn vì agent chưa chạy ở đây.

## MicroVM — căn phòng cách ly trong nhà (legacy, đã thay bằng container — xem ADR-0006)

Máy ảo siêu nhẹ: có **kernel riêng** (não riêng). Dione từng dùng
Cloud Hypervisor, nay dùng podman (share kernel, nhẹ hơn nhiều).
Khác container (container share kernel = share não với host).

## Container — phòng cách ly nhẹ (podman rootless, thay MicroVM từ ADR-0006)

Agent chạy trong container; bind-mount duy nhất là workspace
(`/workspace:rw`), không sờ được host. Share kernel với host nên nhẹ
hơn VM rất nhiều. Không có `podman` → Dione rớt về Host mode + banner.

## KVM — chìa khóa vào phòng (legacy, không cần nữa)

`/dev/kvm` từng cần để tạo MicroVM. Nay không dùng: kiểm tra sandbox
bằng `which podman`.

## virtiofs — cửa sổ lùa cũ (legacy, nay là bind-mount `-v …:rw,Z`)

## vsock — ống nói chuyện host ↔ VM (legacy, nay là `podman exec`)

## SSH — chìa khóa + ống nói (legacy trong Dione, nay là `podman exec -it`)

Mở terminal vào container: `podman exec -it <name> sh`. Không key,
không `authorized_keys`, không reuse gì cả.

## Worktree — bàn làm việc riêng

`git worktree add` checkout 1 branch ra 1 thư mục riêng.
Dione: `<repo>/.dione-worktrees/<slug>` + branch `dione/<slug>`.
1 task = 1 worktree = N agent chạy song song không giẫm file nhau.

## Workspace — cả tầng làm việc

1 repo + cấu hình + worktrees. Dione: **1 container / 1 worktree**
(lazy, worktree rảnh → `pause`) nhưng bind-mount chung repo root
`/workspace:rw` (worktrees vẫn share `.git`).

## Quota / Usage — hạn xài và đo xài (M9)

Tokens thật (đo local: codexbar-style JSONL) + %quota cửa sổ 5h khi có
limit; tiền $ chỉ khi server báo (opencode) — thiếu thì `n/a`/`~$` lower-bound.

## Agent — người thợ trong phòng

CLI bất kỳ chạy trong terminal (Claude Code, Codex, OpenCode, Gemini…).
Dione không bundle agent, không khóa agent mặc định (BYO).
Thêm agent mới = thêm 1 kit script (xem `AGENT-ANY.md`).

## Kit script — công thức lắp thợ

Script cài agent lúc boot container (`kits/claude.sh`…). Image gốc minimal,
agent luôn mới nhất mà không build lại image. Học từ Docker `sbx` kits.

## Backend/Provider — ổ cắm thay được

Interface (trait) + nhiều implementation: `WorkspaceProvider { Host,
Podman }`, `AgentBackend { Opencode, Terminal }`.

## Transcript — biên bản cuộc họp

`UnifiedMessage { role, text, tool, ts }` thay cho `Message/Part` của
opencode trong UI. Mọi agent đều dịch về 1 biên bản chung để Chat render.

## Secrets proxy — két sắt ở nhà chính

API key nằm ở keychain host. Khi agent cần, host bơm vào env của lệnh
exec, không ghi file trong container. Agent gọi được API nhưng không đọc được key.

## NetPolicy — nội quy mạng

`Open` (cho hết, p1; container chạy `--network slirp4netns`, open egress).
`Balanced`/`Locked` là ý tưởng tương lai — code chưa có enum nào cả.

## Dispatcher/kanban — quản đốc

Vòng lặp mỗi 60s: thu hồi task kẹt (stale/crash), giao việc, retry có giới
hạn (`failure_limit=2 → blocked`). Học Hermes kanban.

## Memory (M12) — sổ tay của quản đốc (orchestrator, không chấm model)

`RepoMemory` per-repo, cap 50 entries FIFO (in-memory; disk để sau). Mỗi
`MemoryEntry { task, slug, agent_ref, kind: Win/Fail/Note, text≤140, ts }`
được `distill_entry(&Task, kind, ts)` rút ra thuần túy: chỉ lấy dòng đầu
`task.summary` (trim) hoặc fallback `slug`, cắt 140… + `…`, không LLM/không
bịa. `propose_agents_patch(&RepoMemory, take)` render snippet `## Dione
Memory` + `<!-- dione:memory:* -->` bullets để human duyệt trong File/Diff
viewer (`suggest-only`, không tự ghi `AGENTS.md`); `recall_context(&RepoMemory,
take)` trả block `"- [Kind] slug: text"` để nối vào `Task.summary`/composer
khi mở child/retry. Scorers/benchmarks A/B model đã loại — ADE điều phối
chứ không test agent.

## Factory — dây chuyền

`factory.yaml` as-code: triggers/agents/gates check-in vào repo.
Học Warp Factories. Đo $/PR, evals, benchmarks.
