---
name: dione-wire
description: opencode 1.18 wire shapes and SSE discipline for base runtime work
---

# ADE wire notes

Load this skill when touching `crates/base` runtime/state/server.

## SDK

`opencode-codes 1.18` (`async-client`, `server`). Client:
`create_session`, `prompt_async`, `list_messages`, `abort`,
`respond_permission` (deprecated upstream but working),
`request` (escape hatch), `event_stream`. `ManagedServer::builder()` for
`opencode serve`. `portpicker` is NOT re-exported — depend directly.

## Verified shapes (against vendored 1.18.19 source)

- `SessionStatus` is `#[serde(tag = "type")]`: `{"type":"busy"}`.
- Event envelope uses `properties` (not `data`); `Event` is `tag = "type"`.
- `Event::SessionStatus(EventSessionStatus)` → `properties: SessionStatus2Data
  { session_id, status }`. `session.idle` → `SessionIdleData { session_id }`.
- `Event::PermissionAsked` → `PermissionAskedData { id, session_id,
  permission, patterns, metadata, always, tool }`.
- `Event::PermissionReplied` → `PermissionRepliedData { request_id, ... }`.
- `Event::MessageUpdated` → `{ info: Message, session_id }`;
  `Message` is `tag = "role"` (`user`/`assistant`).
- `Event::MessagePartUpdated` → `{ part: Part, session_id, time }`.
- `Event::TodoUpdated` → `{ session_id, todos }`.
- `StreamEvent` / `Event` are `#[non_exhaustive]` → wildcard arm required.

## SSE discipline

SSE is best-effort. Always reconcile via REST (`list_messages`) on interval
and after `StreamEvent::Connected`. `Store::apply_event` is the only
event-to-state path; keep it total and panic-free.
