# WORKSPACE-CONTAINER Spec (thay WORKSPACE-VM từ ADR-0006)

Sandbox = podman rootless, không MicroVM. Code theo file này.

## Khái niệm

- `Workspace` = 1 repo + cấu hình + worktrees. ID = canonical path của repo.
- `WorkspaceProvider` = mặt chung cho Host và Podman (xem ARCHITECTURE-v2).
- 1 container / 1 workspace. Worktrees nằm trong bind-mount,
  không phải mỗi worktree 1 container.

## ContainerSpec

```rust
struct ContainerSpec {
    name: String,         // "dione-<12hex>" từ hash canonical path
    host_root: PathBuf,   // bind-mount read-write vào /workspace
    image: String,        // "docker.io/library/ubuntu:24.04" (pin digest)
    preview_host: Option<u16>, // container :3000 → host 41xx
}
```

Hardening cố định lúc `run` (không flag tùy ý): `--cap-drop=all`,
`--security-opt=no-new-privileges`, `--pids-limit=256`, `--memory=2g`,
`--cpus=2`, `--read-only` + `--tmpfs /tmp`, `-w /workspace`,
`--hostname ade`, `--network slirp4netns` (open egress).
Cấm: `--privileged`, `--pid=host`, `--net=host`, mount ngoài root.

## State machine (enum dùng chung cho UI + code)

```
Missing → Pulling → Running → Stopped
              │         │
              └── fail ─┴──→ Error(banner + giữ worktree retry tay)
```

Không `WaitingSsh`/`Mounting` — không guest SSH, không virtiofs.
UI Fleet hiển thị badge state.

## Lifecycle (checklist cho implement)

1. `probe`: `which podman` có? Không → fallback `HostProvider` + banner.
2. `ensure`: `ps` thấy container → giữ nguyên. Thiếu → `pull` (có sẵn
   thì skip) → `run -d … sleep infinity` → `Running`.
3. `exec`: `podman exec -e K=V -w /workspace/<rel> <name> <cmd>`.
   Secrets bơm qua `-e`, không copy file key vào container.
4. `shell`: pty local chạy `podman exec -it -w <dir> <name> sh`.
5. `stop/prune`: `rm -f <name>` — xóa container, worktrees đã merge
   thì prune branch.

## Bind mount

- Read-write, sync 2 chiều tức thì (`-v <root>:/workspace:rw,Z`).
  `git_diff`/`merge` chạy trên mount nên host thấy ngay.

## Live requirements (máy chạy container thật)

- Binary: `podman` (rootless), network để `pull` image lần đầu.
- Live test: `DIONE_LIVE_PODMAN=1 cargo test -p workspace --test live_podman`
- Không podman → test SKIP-pass, app Host mode, CI/Xvfb vẫn xanh.

## Giới hạn p1 (không làm)

- Không Docker-in-container (để M14+).
- Không GPU passthrough.
- Không Balanced/Locked enforce.
