# ADR 0007: 1 container / 1 worktree với pause/unpause (lazy + nhẹ)

Ngày: 2026-09-17. Trạng thái: chấp nhận. Supersedes: ADR-0002 (về scope), builds on: ADR-0006.

## Bối cảnh

ADR-0002 chọn 1 VM / 1 workspace để tiết kiệm RAM/CPU khi mỗi VM là
Cloud Hypervisor. Sau khi chuyển sang podman (ADR-0006), chi phí mỗi
container giảm mạnh nhưng yêu cầu mới là **nhẹ mặc định**: app mở không
chạy podman cho tới khi worktree thực sự cần agent/terminal; worktree
không active nên ngủ để tiết kiệm.

1 container / workspace mâu thuẫn với "chỉ worktree đó chạy" —
cần phân rã theo worktree để pause từng worktree riêng.

## Quyết định

- **1 container / 1 worktree.** Key là worktree path
  (`<repo>/.dione-worktrees/<slug>`), name `dione-<12hex>` từ hash
  canonical worktree path (hàm `container_name_for` cũ tái dùng).
  Mount vẫn `-v <repo-root>:/workspace:rw,Z` (giữ git metadata worktree
  trỏ về repo mẹ), nên tạo/xóa worktree vẫn là `git worktree` trên host.
  Worktree-only mount để future (tách mount per-worktree).
- **Lazy:** mở/select worktree → `ensure_running` worktree đó (pull nếu
  thiếu → run → Running). Không auto-ensure khi boot app; không podman
  → Host fallback + banner (kế thừa ADR-0006).
- **Sleep nhẹ:** worktree rảnh → `podman pause` (cgroup freezer, giữ
  container + mount, dậy ~100ms). Quy tắc: chỉ pause container idle
  (không có live task `inbox.has_slug` và không có session Busy/Retry
  trong scope). Có agent đang chạy → không pause để khỏi đóng băng.
- **Wake:** select lại worktree → `podman unpause` nếu Paused, else
  `ensure_running`. Nút ⏻ giữ làm manual ensure/override; × vẫn
  `rm -f`.

## Vì sao không giữ 1 container / workspace

- Pause per-workspace thì worktree khác trong cùng workspace không thể
  chạy riêng (mâu thuẫn "chỉ worktree đó chạy").
- Vfs rootless: mỗi container copy rootfs; nhưng N nhỏ (2–5) và image
  ubuntu:24.04 cache sẵn. Overlay trên máy thật rẻ hơn. Chi phí pause
  rẻ hơn nhiều so với `rm -f` rồi `run` lại.

## Hệ quả

- `ContainerManager` thêm `pause`/`unpause` + state `Paused`. `Store`
  không thêm field (state vẫn ở `DioneApp` như VM state trước đây).
- `ContainerThread` thêm `Wake`/`SleepIdle` và policy idle-only; key
  chuyển từ workspace-root sang canonical worktree path.
- `sandbox/` crate riêng để future (tách khỏi `crates/workspace`), chưa
  làm ở slice này — podman vẫn ở `workspace/{podman,containers}.rs`.
- Docs: mount-root decision và pause-idle gate ghi rõ để LABS sau này
  khỏi hỏi "sao không pause container đang busy".
