# ADE Status (living file — update at the end of every task)

## Current milestone

M4 DONE (seam + wiring + tests; production driver pending → M6).
M5 lõi DONE (CH-only, live-gated). Next: M6 Terminal.

## Last commit (đã verify)

- Wiring runtime (hook A sau reconcile, trước publish), fleet rỗng =
  no-op cho tới khi driver M6 đăng ký tasks:
  - `fleet.rs` seam: `SupervisedTask` (+`slug`/`unbind`) + `TaskSweeper`
    (task-based) + `sweep_due`/`drain_fleet`/`apply_sweep`/`bind_new_session`
    /`release_sessions` — `LoopState` thêm `fleet`/`sweeper`, không dep ngược
  - `create_session_in` bind/track theo slug; `drop_scope` unbind/untrack
    trước khi retire
  - `Supervisor: SupervisedTask` (+`slug`); `AgentBackend::{collect,
    unbind_session}` (adapter override: `collect_new` + reverse-lookup)
  - `FleetSweeper` adapts `Dispatcher` (vá bug `Default` zero-interval);
    consumer Reclaim/Blocked → error entries
  - Lifecycle: `retire` + `SessionDeleted` drop transcripts/costs/pending +
    `recompute_totals` (giới hạn: msg supervisor dưới task id khác vẫn orphan)
  - UI: slow-refresh agents/KVM mỗi ~60 polls; lab-ready: `agents.toml.example`
    + `kits/opencode.sh` skeleton, Lab 3/4 ghi rõ BLOCKED tới đâu
- Verified: `cargo check --workspace` + clippy 0 warnings + `cargo fmt`
  sạch + `cargo test --workspace` 103 passed (0 failed).

## Trước đó (M5, đã verify)

- `090e4e8` → `b4155a6` feat(m5b/m5f): `ExternalSbxBackend` scaffold +
  fake-shim + live tests `ADE_LIVE_SBX=1` (validate sớm VmManager/Provider)
- `4092f7a` feat(m5c): `EphemeralKey` (ssh-keygen, wipe on drop) + image
  pull/verify (curl resume + sha256; test bắt bug verify-bool bị nuốt)
- `d09d6a3` feat(m5d): `VmManager` (key canonical path, timeouts boot
  30s/ssh 20s/mount 10s, Error giữ retry tay) + `ssh_info` — 3 tests stub
- `efc6be1` feat(m5e): `MicroVm` provider (exec ssh quoted, secrets
  env-prefix, PathMapping Identity/Mounted) — 5 tests fake-ssh
- `8c05ebb`/`c1c602e`/`fc2b636`/`7d54fbe` feat(m5g): UDS HTTP client +
  vsock proxy (libc AF_VSOCK, bridge test) + cloud-init seed +
  `CloudHypervisorBackend` (create/boot/shutdown/delete/info,
  virtiofsd spawn, Drop kill)
- `2345604` feat(m5h): live test `ADE_LIVE_VM=1` (Lab 2 boot + Lab 3
  mount 2 chiều, SKIP-pass mặc định)
- `a3bb7bc` feat(m5i): UI `vm_badge` (label/dot + Fleet badge + banner
  Host mode khi mất KVM, Lab 6) — phát hiện pitfall `#[test]` vs
  `use gpui::*` (đã ghi vào GPUI.md)
- Xóa sbx (ADR-0005: thiếu virtiofs/vsock/image controls) — CH duy nhất
- Verified: `cargo check --workspace` + clippy 0 warnings + `cargo fmt`
  sạch + `cargo test --workspace` 78 passed (core 14 + agent 5 + vm 25
  + live_ch 1 + workspace 11 + state 13 + worktree 7 + ui 2),
  Xvfb smoke 68s clean.
- Chưa live-test thật (máy này không KVM/sbx): các slice live gắn gate
  env, chạy trên máy đủ điều kiện khi có.

## Next up

1. Live lần đầu trên máy KVM: Lab 2/3 (`ADE_LIVE_VM=1`), re-verify flag
   virtiofsd + vsock socket + user `ubuntu` + cmdline root.
2. M4 còn lại: driver production đăng ký `Supervisor`/`FleetSweeper` vào
   runtime (M6: Open-Workspace flow tạo tasks thật) — seam + wiring xong.
3. M6: Open-Workspace flow (nối VmManager vào UI) + local pty +
   `TerminalAdapter` + kit đầu.
4. Tier B live prompt: opt-in, tốn quota, chạy tay khi cần.

## Blockers

- None. Cần 1 máy có `/dev/kvm` để chạy live tests M5 lần đầu.
