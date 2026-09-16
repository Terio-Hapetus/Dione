# LABS — bài thực hành copy-paste được

Mỗi lab có: mục đích → lệnh → kết quả đúng → hỏng thì sao. Lab đỏ ở đâu,
đọc spec ở đó.

## Lab 0: máy có podman không

```bash
which podman
```

- Thấy path → làm được container labs.
- Không thấy → app vẫn chạy Host mode (banner "Podman unavailable"). Làm labs
  Host trước, container labs để sau. Xem `WORKSPACE-VM.md#lifecycle` bước probe.

## Lab 1: worktree cơ bản (M2, Host, không container)

```bash
cargo test -p base worktree
```

- Đúng: tests `create/remove/prune/merge/dirty-guard` xanh.
- Đỏ: đọc `ARCHITECTURE.md` (Store/worktree) + `WORKFLOW.md#khi-bị-kẹt`.

## Lab 2: hello-container (cần podman)

```bash
cargo test -p workspace --lib   # chưa podman cũng xanh (fakes)
# Có podman (xem WORKSPACE-VM.md "Live requirements"):
# DIONE_LIVE_PODMAN=1 cargo test -p workspace --test live_podman -- --nocapture
```

- Đúng: `Missing → Pulling → Running`, exec `touch` thành công.
- `Error`: kiểm tra image đã pull? `podman ps -a` thấy container?
  Xem `WORKSPACE-VM.md#lifecycle`.

## Lab 3: mount thấy file 2 chiều (cần podman + live container)

> Terminal tab mở container shell khi badge `running` (toggle `Term`).
> Lab này mở khi live Lab 2 xanh.

Trong container (qua Terminal tab):

```bash
touch /workspace/hello-from-ctr && echo ok
```

Trên host:

```bash
ls <repo>/hello-from-ctr
```

- Thấy file → bind-mount ok.
- Không thấy → kiểm tra `-v <root>:/workspace:rw,Z` lúc `run`
  (xem `WORKSPACE-VM.md#bind-mount`).

## Lab 4: agent mới (không container cũng chạy được)

```bash
mkdir -p ~/.config/dione && cp agents.toml.example ~/.config/dione/agents.toml
which opencode   # + which claude / codex nếu đã cài
cat ~/.config/dione/agents.toml
cargo test -p workspace agents && cargo test -p desktop top_bar
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

## Lab 6: rớt mạng / mất podman (fallback)

- Tắt mạng hoặc gỡ `podman` khỏi `PATH` (mô phỏng) → app phải hiện banner,
  giữ worktree, cho retry tay. Không crash.
