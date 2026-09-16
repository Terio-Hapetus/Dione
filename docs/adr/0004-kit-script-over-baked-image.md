# ADR 0004: Kit script lúc boot thay vì bake agent vào image

Ngày: 2026-09-05. Trạng thái: chấp nhận.

## Ngữ cảnh

Agent mới ra liên tục (Claude/Codex/OpenCode…). Bake vào image thì mỗi lần
update phải build lại image nặng. Học `sbx` templates/kits.

## Quyết định

Image gốc **minimal** (kernel + Ubuntu + SSH + deps). Agent cài lúc boot
bằng **kit script** (`kits/<ten>.sh`), versioned riêng.

## Vì sao

- Thêm agent mới = thêm 1 file script + 1 dòng `agents.toml` (xem
  `AGENT-ANY.md`), không đụng image/VM/core.
- Luôn cài được bản mới nhất; rollback = pin version trong script.
- Phù hợp solo non-SWE: sửa shell script dễ hơn build image.

## Hệ quả

- `kits/` + `agents.toml` là registry agent, có lab probe riêng.
- Image versioned riêng (`dione-ubuntu-24.04:vX`), checksum verify lúc pull.
- Không cache credentials trong image/layer.
