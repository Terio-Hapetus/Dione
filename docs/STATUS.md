# ADE Status (living file — update at the end of every task)

## Current milestone

M4 DONE (seam + wiring + tests; production driver pending → M6).
M5 lõi DONE (CH-only, live-gated).
M6 Terminal Host-only DONE (driver + pty + adapter + kit + ssh-shell).
M6-VM W1–W3 DONE (driver provider ngoài, VmManager-thread, SSH tab).
M5-live prep DONE (probe readable, user ubuntu, image max-time).
M7a DONE (per-task retry budget + error→sweep→block chain, Host-only).
M7b-core DONE (Blocked persist + untrack + RetryTask, chưa UI).
M7b-UI DONE (badge + nút ↻ row/strip + Xvfb smoke).
M7c DONE (handoff minimal: parent + summary, data-only UI).
M7d DONE (base origin/HEAD offline-safe + cấm trùng slug).
M7 CLOSED (retry budget + handoff + hygiene, Host-only).
M8a DONE (hunk cherry-pick core + Diff tab picks, trên branch M7).
M8b0 DONE (render git-shape diffs — cherry-pick sống trên diff thật).
M8b DONE (review queue: NeedsYou trước + chờ-lâu-nhất, fair queue).
Next: M8c (annotate multi-line) → M8d (branch paths) → M8e (viewer);
push/PR chờ fix gh auth.

## Last commit (đã verify)

- M8b (`c78e536` + `1c910b0`):
  - M8b0: `file_rows` đọc cả git-shape `{raw,files}` → 1 file block
    (fix diff git-first hiện 0 file; cherry-pick M8a sống lại) + 3 tests
  - M8b: `review_rank/sort_review_sids/sort_review_scopes` — NeedsYou
    trước, rồi updated cũ nhất; main cạnh tranh bình đẳng; sid mồ côi
    chìm đáy; thay `sids.sort()` + main-first cũ trong `render_diff`
  - Tests: queue order/scope-fair/sink-unknown (state_tests) + Xvfb smoke
- Verified: check + clippy 0 + fmt + `cargo test -p ade-core -p ade-ui`
  xanh + Xvfb smoke (cửa sổ hiện, log sạch).
- M8a (`41ca7d1`, trên branch `feat/m7-fleet-reliability`):
  - Core: `Hunk` + `split_hunks` (pure) + `apply_hunks` (dựng patch tối
    thiểu → `git apply` qua stdin; rỗng = no-op; conflict fail sạch)
  - `Command::ApplyHunks { file, hunks }` → apply vào main checkout +
    báo kết quả qua error strip
  - Diff tab: toggle `☐/☑` mỗi dòng `@@` + nút `Apply N hunk(s) → main`
    (picks trong `AdeApp.hunk_picks`, clear sau khi gửi)
  - Tests: split/subset/conflict/bad-input (worktree 13/13)
- Verified: check + clippy 0 + fmt + tests 4 crates xanh + Xvfb smoke
  (cửa sổ hiện, log sạch). Lưu ý: 2 lần fail lẻ `microvm` khi 4 crate
  chạy song song, pass 3/3 riêng lẻ + full rerun — flake timing fake-ssh
  có sẵn, không đụng code slice này.
- M7d (`44eec2d`, Host-only, offline-safe):
  - `worktree::resolve_base`: `origin/HEAD` nếu có local, fallback `HEAD`
    (không fetch); `create()` pin `worktree add -b <branch> <base>`
  - `FleetInbox::{has_slug, register_task -> bool}` + driver bail
    `task already open` trước khi dựng backend (không side-effect)
  - Tests: pin-base qua remote bare local (A vs B), fallback HEAD,
    dup slug inbox + driver
- Verified: check + clippy 0 + fmt + `cargo test -p ade-core
  -p ade-agent -p ade-workspace` xanh (worktree 9/9).
- M7c (`2b73118`, data-only UI):
  - `Task { parent, summary }` + `with_parent/with_summary` + `child()`
    (id/budget mới, kế thừa summary + limit; grandchild trỏ cha trực tiếp)
  - `handoff_summary` (3 dòng cuối `role: text`, cap 500) — Supervisor tự
    snapshot khi vừa chuyển `Blocked`, không đè summary tay
  - Seam `TaskHandoff` + `SupervisedTask::handoff()` + driver
    `open_child_task`; Reclaim vẫn log-only (auto-respawn chờ callsite
    production `rt.fleet()`)
  - Tests: child/parent/summary, tail+cap, snapshot/đè-tay, seam, child flow
- Verified: check + clippy 0 + fmt + `cargo test -p ade-workspace
  -p ade-agent -p ade-core` xanh.
- M7b-UI (`aabacc1`):
  - Pure helpers `fleet_dot` (blocked → đỏ, đè mọi state) + `blocked_task_in`
    (parse TaskId từ `fleet: blocked {id}`) + 3 tests
  - Nút `↻` mỗi worktree row bị blocked (gửi `Command::RetryTask`) + nút
    `↻ retry` ở error strip (resolve slug từ message → `store.blocked`;
    tự ẩn sau retry)
  - Verified: `cargo test -p ade-ui` 13 passed + Xvfb smoke (cửa sổ
    `ADE — Agentic IDE` 1440x900 hiện, log sạch)
- M7b-core (`3427842`, Host-only, chưa UI):
  - `Store.blocked: BTreeMap<TaskId, slug>` + `mark/is/clear_blocked_by_slug`
    (in-memory; restart rebuild từ sweep mới)
  - Sweep ghi slug (resolve từ inbox tasks) + untrack Blocked → hết spam lặp
  - `Command::RetryTask { slug }` → `inbox.retry_task` (reset + re-track
    đúng limit) + xóa mirror; slug lạ → lỗi rõ
  - Tests: mirror round-trip, slug mapping, quiet-sau-block, retry→silent,
    `supervisor_retry_reset`
- Verified: `cargo check --workspace` + clippy 0 warnings + `cargo fmt`
  sạch + `cargo test -p ade-core -p ade-agent -p ade-workspace` xanh.
- M7a retry budget (`f61982f` + `2421649`, Host-only, không KVM vẫn xanh):
  - `Task { failure_limit = 2, failure_count, status: Active/Retrying/Blocked }`
    + `with_failure_limit`/`note_failure`/`note_success`/`retry_reset`
  - `Dispatcher` limit-aware (`track` copy limit, `track_id_with_limit`;
    hết hardcode `>= 2`); `FleetSweeper` forward limit
  - Error chain: `Supervisor::tick` thu `AgentStatus::Error` →
    `drain_errors` → `poll_fleet` → `note_task_error` → sweep `Blocked`;
    `bind_new` forward `failure_limit`; `FleetInbox: Default` (fix clippy
    `new_without_default` có sẵn)
  - Tests: budget default/custom/clamp, sweep threshold, forward errors,
    chain đầy đủ inbox→sweep→blocked (`inbox_chain_blocks_erroring_task…`)
- Verified: `cargo check --workspace` + clippy 0 warnings + `cargo fmt`
  sạch + `cargo test -p ade-workspace -p ade-agent -p ade-core` xanh.
- Trước đó W1–W3 + M5-live prep (không KVM vẫn xanh, 132 passed):
  - `open_task_with` (provider ngoài + nhánh terminal) — Lab 4 fan-out
    headless xanh; callsite production (`rt.fleet()`) còn lại
  - `VmThread` (Mock/CH theo probe, keys + preview ports có chủ) báo
    `VmState` về badge qua channel; nút ⏻/Stop mỗi worktree; Xvfb sạch
  - SSH tab chọn Host-vs-guest (`Ready` → `MicroVm::shell` + `-L`,
    endpoint hiện trong badge); A1 probe readable, A2 user ubuntu,
    A3 image `--max-time` + runbook live Lab 2

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
