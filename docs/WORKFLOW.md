# WORKFLOW — cách làm việc mỗi ngày (dành cho người vừa học vừa làm)

## Vòng lặp solo (research 30% → slice <3 ngày → verify)

1. **Research:** đọc `00-START-HERE` → file spec liên quan → `LABS.md` lab gần nhất.
2. **Slice nhỏ:** 1 ý duy nhất, <300 dòng (mẫu chuẩn M2c `275`, M2e `154`).
   Đặt tên `feat(mXy): <động từ> + phạm vi`. Không gộp docs vào feat.
3. **Verify (trước mọi commit):**
   ```bash
   cargo check --workspace
   cargo clippy --workspace --all-targets   # 0 warnings
   cargo fmt --all
   cargo test -p base
   # Nếu đụng UI:
   env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET xvfb-run -a -s "-screen 0 1440x900x24" ./target/debug/dione
   ```
4. **Kết slice:** `git status` → stage đúng file → commit → update `STATUS.md`
   (position/next/blockers).

## Khi thêm module mới (chống vỡ legacy)

- Viết trait + `Mock` trước, test `Mock` xanh rồi mới viết backend thật.
- Module mới sống ở crate đúng tầng (`base` = wire/types dùng chung,
  `workspace` = task/container/memory, `agent` = backend/supervisor;
  file lớn hiện tại: `supervisor 700+`, `app 600+` — tách khi sửa sâu).
- Không sửa signature cũ: thêm types mới + adapter translate ở biên.
- Build mặc định phải qua được trên máy không podman (Host mode + banner).

## Khi bị kẹt

- Chạy lab gần nhất còn xanh trong `LABS.md`. Lab đỏ ở đâu → đọc spec ở đó.
- Hỏi 3 câu: (1) UI hay core? (2) Host hay container? (3) Agent nào? Thu hẹp rồi mới sửa.
- Slice >300 dòng → tách tiếp (kiểu M1a `1190` vỡ là bài học, đừng lặp lại).

## Checklist cuối task

- [ ] `cargo test -p base` xanh (40+ tests; full `--workspace`: base 40 + agent 48 + workspace 79 + desktop 38)
- [ ] clippy 0 warnings, `cargo fmt` đã chạy
- [ ] `STATUS.md` đã update
- [ ] Lab liên quan trong `LABS.md` còn xanh
