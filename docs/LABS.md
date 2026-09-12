# LABS — bài thực hành copy-paste được

Mỗi lab có: mục đích → lệnh → kết quả đúng → hỏng thì sao. Lab đỏ ở đâu,
đọc spec ở đó.

## Lab 0: máy có KVM không

```bash
ls -l /dev/kvm
```

- Thấy `crw-rw-rw- … /dev/kvm` → làm được VM labs.
- Không thấy → app vẫn chạy Host mode (banner "VM unavailable"). Làm labs
  Host trước, VM labs để sau. Xem `WORKSPACE-VM.md#boot-sequence` bước probe.

## Lab 1: worktree cơ bản (M2, Host, không VM)

```bash
cargo test -p ade-core worktree
```

- Đúng: tests `create/remove/prune/merge/dirty-guard` xanh.
- Đỏ: đọc `ARCHITECTURE.md` (Store/worktree) + `WORKFLOW.md#khi-bị-kẹt`.

## Lab 2: hello-vm (M5, cần KVM)

```bash
cargo test -p ade-vm --lib   # chưa KVM cũng xanh (Mock + fakes)
# Có KVM (xem WORKSPACE-VM.md "Live requirements"):
# ADE_LIVE_VM=1 cargo test -p ade-vm --test live_ch -- --nocapture
```

- Đúng: log `Booting → WaitingSsh → Mounting → Ready` <60s.
- Timeout SSH: kiểm tra image đã pull? key ephemeral đã bơm? Xem
  `WORKSPACE-VM.md#boot-sequence` bước 3–4.

### Live lần đầu — checklist (chạy trên máy `/dev/kvm`)

1. `ls -l /dev/kvm` phải readable (không thì app fallback Host mode).
2. Binaries: `which cloud-hypervisor virtiofsd genisoimage ssh curl
   sha256sum ssh-keygen` — thiếu cái nào thì live fail ở bước đó.
3. Assets (1 trong 2): `ADE_VM_KERNEL` + `ADE_VM_IMAGE` trỏ file local,
   hoặc `ADE_VM_*_URL` (+`_SHA256`) để first-boot download vào
   `~/.local/share/ade/vm/` (curl `--max-time` 600s mặc định).
4. Chạy live test, đọc lỗi theo lớp: assets → spawn binaries →
   `api.sock` → `vm.create/boot` (xem body daemon) → ssh-timeout
   (seed socat/sshd, user `ubuntu`, `BatchMode`) → mount 2 chiều.
5. Flag virtiofsd/vsock socket/cmdline root/user nếu sai thì sửa code
   (có NOTE trong `ch.rs`) rồi chạy lại — đừng sửa test cho qua.
6. `ADE_LIVE_KIT=1` hiện chưa wired test nào — kit mới chỉ có
   fake-guest tests; live kit là việc riêng sau live VM xanh.

## Lab 3: mount thấy file 2 chiều (cần KVM + live guest — SSH tab đã có)

> SSH tab đã nối guest ở W3 (toggle `Term` khi badge `ready`, endpoint
> hiện trong badge để SSH tay). Lab này mở khi live Lab 2 xanh.

Trong VM (qua SSH tab):

```bash
touch /workspace/hello-from-vm && echo ok
```

Trên host:

```bash
ls <repo>/hello-from-vm
```

- Thấy file → virtiofs ok.
- Không thấy / `git status` lạ → `VIRTIOFS_CACHE=0` rồi remount
  (xem `WORKSPACE-VM.md#virtiofs-mount`).

## Lab 4: agent mới (không VM cũng chạy được)

```bash
mkdir -p ~/.config/ade && cp agents.toml.example ~/.config/ade/agents.toml
which opencode   # + which claude / codex nếu đã cài
cat ~/.config/ade/agents.toml
cargo test -p ade-workspace agents && cargo test -p ade-ui top_bar
```

- Thấy binary + entry trong `agents.toml` → Agent picker hiện tick xanh
  (`●`), thiếu binary → tick đỏ (`○`); chưa có file → picker trống.
- Nửa sau của lab (fan-out 1 prompt → 2 tasks → diff cả 2) còn BLOCKED:
  driver `open_task_with` đã có + tests xanh nhưng chưa có callsite
  production (chưa có nút/command nào gọi `rt.fleet()` trong app).

## Lab 5: fan-out + merge (Host, M2)

1. Tạo 2 worktrees (`+ wt` trong app).
2. Gửi 1 prompt bằng `⇉ all`.
3. Compare diff (`↻ all`), annotate 1 dòng, gửi về đúng agent.
4. Merge winner, prune. Worktree dirty phải được giữ lại + báo lỗi rõ.

## Lab 6: rớt mạng / mất KVM (fallback)

- Tắt mạng hoặc rename `/dev/kvm` (mô phỏng) → app phải hiện banner,
  giữ worktree, cho retry tay. Không crash.
