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

## Lab 3: mount thấy file 2 chiều

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
which claude || which codex || which opencode
cat ~/.config/ade/agents.toml
```

- Thấy binary + entry trong `agents.toml` → Agent picker hiện tick xanh.
- Thêm agent mới: theo `AGENT-ANY.md#thêm-agent-mới-trong-3-bước`,
  rồi fan-out 1 prompt → 2 tasks → diff cả 2 đều hiện.

## Lab 5: fan-out + merge (Host, M2)

1. Tạo 2 worktrees (`+ wt` trong app).
2. Gửi 1 prompt bằng `⇉ all`.
3. Compare diff (`↻ all`), annotate 1 dòng, gửi về đúng agent.
4. Merge winner, prune. Worktree dirty phải được giữ lại + báo lỗi rõ.

## Lab 6: rớt mạng / mất KVM (fallback)

- Tắt mạng hoặc rename `/dev/kvm` (mô phỏng) → app phải hiện banner,
  giữ worktree, cho retry tay. Không crash.
