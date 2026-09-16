# ADR 0005: Bỏ ExternalSbx, Cloud Hypervisor là backend VM duy nhất

Ngày: 2026-09-10. Trạng thái: chấp nhận.

## Ngữ cảnh

M5計画 dùng `ExternalSbx` (gọi `sbx` CLI) làm "đường tắt VM thật sớm",
Cloud Hypervisor làm "đường chính". `sbx` đã được implement ở `m5b`
và dùng để validate `VmManager` + `MicroVm` provider.

## Quyết định

**Xóa `ExternalSbxBackend`.** `vm` chỉ còn `{ CloudHypervisor, Mock }`.

## Vì sao

- `sbx` thiếu các tính năng quản lý MicroVM nâng cao (virtiofs tags,
  vsock CID, image pinning) nên khó kiểm soát đúng spec WORKSPACE-VM.
- Giữ 2 backend thật = nhân đôi ma trận test live mà lợi ích duy nhất
  (có VM thật sớm) đã đạt được ở bước validate.
- Giàn giáo đã hoàn thành vai trò: fake-shim tests ghim cú pháp CLI,
  live tests `DIONE_LIVE_SBX=1` còn trong git history (`090e4e8`,
  `6d20628`) để tham khảo khi cần.

## Hệ quả

- Máy dev cần VM thật phải có KVM + `cloud-hypervisor` + `virtiofsd` +
  `genisoimage` (xem WORKSPACE-VM.md "Live requirements").
- Không KVM → `MockBackend` + Host mode + banner, CI/Xvfb vẫn xanh.
