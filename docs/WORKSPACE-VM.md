# WORKSPACE-VM Spec (code theo file này ở M4–M6)

## Khái niệm

- `Workspace` = 1 repo + cấu hình + worktrees. ID = canonical path của repo.
- `WorkspaceProvider` = mặt chung cho Host và MicroVm (xem ARCHITECTURE-v2).
- 1 VM / 1 workspace. Worktrees nằm trong mount, không phải mỗi worktree 1 VM.

## VmConfig

```rust
struct VmConfig {
    vcpu: u8,          // mặc định 2
    mem_mb: u32,       // mặc định 2048
    image: ImageRef,   // ví dụ "ade-ubuntu-24.04:v1"
    mount_repo: PathBuf,
    net: NetPolicy,    // p1 = Open
    ssh_pubkey: String, // ephemeral, sinh mỗi boot
}
```

## State machine (enum dùng chung cho UI + code)

```
Missing → PullingImage → Booting → WaitingSsh → Mounting → Ready → Running → Stopped
              │              │           │             │
              └──── fail ────┴── timeout ┴── mount-err ┘──→ Error(banner + giữ worktree retry tay)
```

- Mỗi state có timeout riêng (boot 30s, ssh 20s, mount 10s).
- UI Fleet hiển thị badge state; log chi tiết ở Inspector tab.

## Boot sequence (checklist cho implement)

1. `probe`: `ls /dev/kvm` tồn tại + readable? Không → fallback `HostProvider`.
2. `pull`: image thiếu → tải + verify checksum. Có sẵn → skip.
3. `boot`: gọi `VmBackend::boot(cfg)` (CloudHypervisor REST / Mock).
4. `wait_ssh`: poll `wait_ssh` tới timeout. Key ephemeral, không reuse.
5. `mount`: virtiofs mount `mount_repo` vào `/workspace`. Verify `touch` 2 chiều.
6. `ready`: tạo `.ade-worktrees/` nếu thiếu, mở Terminal tab (ssh).
7. `stop/prune`: `vm.shutdown` + `vm.delete` — xóa VM + contents, worktrees đã merge thì prune branch.

## SSH & ports

- SSH server: OpenSSH trong guest, `authorized_keys` bơm lúc boot.
- Host nối qua vsock/port-forward: `ssh -i <ephemeral> -p <port> vm@127.0.0.1`.
- Preview: `VM:3000 → host:41xx` (mỗi workspace 1 dải, tránh đụng).
- Secrets: không copy file key vào VM. Host bơm vào env của `exec`.

## virtiofs mount

- Read-write, sync 2 chiều tức thì. `git_diff`/`merge` chạy trên mount nên host thấy ngay.
- Khi `git status` lạ / file samhäl → thử `VIRTIOFS_CACHE=0` rồi remount.
- Fallback không mount được → git push/pull qua SSH (để M sau, p1 chỉ cần báo lỗi rõ).

## Backend chọn lúc nào

| Backend | Khi nào dùng |
|---|---|
| `Mock` | CI, Xvfb, máy không KVM, test |
| `CloudHypervisor` | Đường duy nhất (xem ADR-0005): native, virtiofs, REST API |

## Live requirements (máy chạy VM thật)

- KVM (`ls /dev/kvm`), binaries: `cloud-hypervisor`, `virtiofsd`,
  `genisoimage`, `ssh`, `curl`, `sha256sum`, `ssh-keygen`.
- Guest assets (1 trong 2 cách):
  - `ADE_VM_KERNEL` + `ADE_VM_IMAGE` trỏ tới file local, hoặc
  - `ADE_VM_KERNEL_URL` (+`_SHA256`) / `ADE_VM_IMAGE_URL` (+`_SHA256`)
    để first-boot download vào `~/.local/share/ade/vm/` (có verify).
- SSH guest qua vsock: cloud-init seed cài `socat` + `openssh-server`,
  forward `VSOCK-LISTEN:2222 → localhost:22`; host proxy giữ port
  127.0.0.1 ngẫu nhiên. Xem `crates/ade-vm/src/seed.rs` + `ch.rs`
  (flag `virtiofsd`/`vsock socket` cần re-verify ở live test đầu tiên).

## Giới hạn p1 (không làm)

- Không Docker-in-VM (để M14+).
- Không GPU passthrough.
- Không Balanced/Locked enforce (chỉ chừa enum + hook proxy).
