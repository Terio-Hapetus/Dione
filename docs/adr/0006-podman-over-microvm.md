# ADR 0006: Podman thay MicroVM (FS-isolation đủ, xóa Cloud Hypervisor)

Ngày: 2026-09-16. Trạng thái: chấp nhận. Supersedes: ADR-0001, ADR-0005.

## Ngữ cảnh

MicroVM (Cloud Hypervisor + virtiofsd + guest kernel + Ubuntu image +
vsock proxy + seed ISO) quá nặng cho solo dev chạy 2–5 agents: cần KVM,
mặc định 2 vCPU/2048MB mỗi workspace, boot 30s/ssh 20s/mount 10s
timeouts, image hàng trăm MB. Thực tế test thấy nặng máy.

Threat model chốt: chỉ cần agent **không thao tác lên host**
(FS/process isolation). Share kernel với host chấp nhận được —
không cần chống kernel-exploit/escape.

## Quyết định

**Podman rootless là engine sandbox duy nhất.** `ade-vm`
(CloudHypervisor, seed, vsock_proxy, uds, image, keys, manager,
backend) + `ade-workspace/src/microvm.rs` bị xóa hẳn. Thay bằng
`PodmanProvider` implement cùng seam `WorkspaceProvider`
(exec/shell/ssh_info=None), 1 container / 1 workspace (giữ ADR-0002),
bind-mount `-v <root>:/workspace:rw,Z` thay virtiofs, `podman exec`
thay SSH, `podman pull` thay image-verify tay.

## Vì sao không giữ CH song song

- Giữ 2 engine thật = nhân đôi ma trận test live (bài học ADR-0005).
- CH chưa từng live-test thật trên máy này (luôn gate `ADE_LIVE_VM=1`,
  máy không KVM) — xóa không mất coverage thật.
- Git history giữ lại toàn bộ CH (`live_ch.rs`, `ch.rs`) để tham khảo.

## Hệ quả

- Mất cách ly kernel: container escape qua kernel-exploit là có thể.
  Ghi nhận, chấp nhận với threat model FS-only.
- Hardening cố định: rootless, `--cap-drop=all`,
  `--security-opt=no-new-privileges`, `--pids-limit`, `--memory`,
  `--cpus`, `--read-only` + `--tmpfs /tmp`, `-w /workspace`,
  `--network slirp4netns` (open egress cho API model/npm/cargo),
  cấm `--privileged/--pid=host/--net=host`/mount ngoài root,
  secrets chỉ `-e` (kế thừa MicroVm env-prefix).
- Không KVM nữa → probe `which podman`, thiếu thì Host fallback +
  banner (thay `probe_kvm`). Không `ssh-keygen`/key ephemeral/cloud-init.
- State rút gọn: `ContainerState{Missing|Pulling|Running|Stopped|
  Error}` — không `WaitingSsh/Mounting`. `Store` không thêm field
  (state container vẫn chỉ ở `AdeApp` như VM state trước đây).
