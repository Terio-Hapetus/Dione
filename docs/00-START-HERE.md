# ADE Docs — bắt đầu từ đâu (5 phút)

Bạn không phải kỹ sư phần mềm? Đọc theo thứ tự này, mỗi file là 1 bước nhỏ.

## Bản đồ

1. `00-START-HERE.md` (file này) — bản đồ tổng.
2. `GLOSSARY-VI.md` — từ vựng: MicroVM, KVM, virtiofs, vsock, SSH, worktree… bằng tiếng Việt + ví dụ đời thường.
3. `VISION.md` — ADE là gì / không là gì (giữ 1 dev + Linux native).
4. `ARCHITECTURE-v2.md` — sơ đồ lớn: Host → Workspace → MicroVM → Agent. Ai gọi ai.
5. `WORKSPACE-VM.md` — spec chi tiết sandbox container (dành cho lúc code).
6. `AGENT-ANY.md` — spec agent-agnostic: thêm agent mới trong 3 bước.
7. `WORKFLOW.md` — cách bạn làm việc mỗi ngày: research → slice nhỏ → verify → update STATUS.
8. `LABS.md` — bài lab copy-paste được để kiểm tra từng bước.
9. `adr/` — 4 quyết định kiến trúc đã chốt + lý do loại phương án khác.
10. `ROADMAP.md` — M0→M15: đã xong gì, sắp làm gì.
11. `STATUS.md` — vị trí hiện tại (living file, update cuối mỗi task).

## Quy ước đọc sơ đồ

- `→` = gọi / phụ thuộc 1 chiều. Không có vòng tròn.
- `Host` = máy bạn đang ngồi. `VM` = máy ảo nhẹ bên trong Host.
- `Workspace` = 1 repo + cấu hình. `Worktree` = 1 bản checkout riêng trong workspace.
- Mặc định: app chạy trên Host (terminal kiểu Warp). Chỉ khi Open Workspace / Run Agent mới boot MicroVM. 1 VM / 1 workspace.

## Khi bị lạc

Mở `LABS.md`, chạy lab gần nhất còn xanh. Lab đỏ ở đâu thì đọc spec tương ứng ở đó.
