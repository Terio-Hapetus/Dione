# ADE Status (living file — update at the end of every task)

## Current milestone

M4 DONE (seam + wiring + tests; production driver pending → M6).
M5 lõi DONE (CH-only, live-gated).
M6 Terminal Host-only DONE (driver + pty + adapter + kit + ssh-shell).
Next: M6 VM-thread wiring + live KVM, rồi M7.

## Last commit (đã verify)

- M6 Host-only (không KVM vẫn xanh; phần VM-thread + live còn lại):
  - `FleetInbox` registration seam (sống sót reconnect) + `open_host_task`
    driver + Lab 4 fan-out Host (2 tasks mock → transcripts)
  - `ShellChannel` + `HostShell` (`portable-pty` 0.9) + terminal tab
    (scrollback viewer + input + search filter), Xvfb smoke sạch
  - `TerminalAdapter` (heuristic Working/Done + `mark_done`, transcript
    strip ANSI) + kit opencode thật (pin/verify, fake-guest tests)
  - `MicroVm::shell` (ssh dưới pty + `-L` forwards) + `PreviewPorts`
    allocator `4100-4199`
- Verified: `cargo check --workspace` + clippy 0 warnings + `cargo fmt`
  sạch + `cargo test --workspace` 126 passed (0 failed).

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
   virtiofsd + vsock socket + user `ubuntu` + cmdline root (+ kit gate
   `ADE_LIVE_KIT=1` khi có).
2. M6 còn lại: Open-Workspace VM-thread wiring (nối `VmManager` vào UI:
   `set_vm_state`/banner hết dead, SSH tab vào guest qua `MicroVm::shell`,
   preview ports) — seam + Host path đã xong.
3. Tier B live prompt: opt-in, tốn quota, chạy tay khi cần.

## Blockers

- None. Cần 1 máy có `/dev/kvm` để chạy live tests M5 lần đầu.
