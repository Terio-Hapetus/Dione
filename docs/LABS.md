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
- Sandbox lồng nhau (máy dev cũng là container: thiếu `/dev/fuse`,
  `/dev/net/tun`, cgroupfs read-only): storage `vfs` + runtime `crun`
  qua env, không đụng code production:

```bash
sudo apt install -y podman slirp4netns crun
sudo mkdir -p /dev/net && sudo mknod /dev/net/tun c 10 200
printf '[storage]\ndriver = "vfs"\nrootless_storage_path = "/tmp/podman-live-root"\n' > /tmp/live-storage.conf
printf '[engine]\nruntime = "crun"\n' > /tmp/live-containers.conf
CONTAINERS_STORAGE_CONF=/tmp/live-storage.conf CONTAINERS_CONF=/tmp/live-containers.conf \
  DIONE_LIVE_PODMAN=1 cargo test -p workspace --test live_podman -- --nocapture
```

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

## Lab 7: Costs tab (M9d, không podman cũng chạy được)

Mở tab `Costs` (bên phải: context/diff/file → costs) sau khi chạy 1 task
bất kỳ (kể cả Host).

```bash
cargo test -p base -p agent -p workspace --lib   # 38 + 40 + 58 xanh (có costs)
```

- Đúng: tổng tokens hiện, breakdown per-agent / per-model có, 5h tokens
  kèm tiền (`~$` = lower-bound khi có sample thiếu tiền, `n/a` = tokens-only).
  Task terminal hiện `n/a` là đúng (không đoán tiền).
- Trống: tab hiện empty-state "No usage yet" cho tới khi có task chạy.
- Đỏ: đọc `ARCHITECTURE-v2.md#core-types`.

## Lab 8: keychain BYOK (M9e, OS keychain)

Tick `◆` (đủ keys) / `◇` (thiếu) cạnh `●`/`○` trong TopBar cho agent có
`env_keys` trong `agents.toml` (ví dụ `env_keys = ["ANTHROPIC_API_KEY"]`).
Values không bao giờ hiện.

```bash
# Ghi 1 key thử (cần GNOME session có secret-service / secret-tool):
secret-tool store --label dione service dione account claude/ANTHROPIC_API_KEY <<<"sk-..."
# Kiểm tra tick → ◆ (đủ); xóa → ◇
secret-tool clear service dione account claude/ANTHROPIC_API_KEY
cargo test -p workspace secrets && cargo test -p desktop top_bar
```

- Đúng: `secret-tool` không có / collection khóa → không crash, ticks không
  có `◆`/`◇` (banner Host mode vẫn làm việc, agent thiếu key tự báo).
- Đỏ: đọc `AGENT-ANY.md#secrets` + `WORKSPACE-VM.md#lifecycle` bước secrets.

## Lab 9: rate-limit (M9f, không podman cũng chạy được)

Mô phỏng backoff của provider: opencode `SessionStatus::Retry` hoặc
pty chứa chuỗi `rate limit` / `429` / `quota exceeded` / `overloaded`.

```bash
cargo test -p base -- --nocapture worktree_status   # Retry → NeedsYou
cargo test -p agent terminal -- --nocapture rate_limit
```

- Đúng: worktree đang backoff vào review queue `NeedsYou` (attention-first)
  chứ không giả `Working`; terminal task latch `NeedsInput` tới `Done` mới hết.
  Composer vẫn khóa khi backoff (đúng, chưa tiến triển).
- Đỏ: đọc `AGENT-ANY.md` bảng TerminalAdapter.
