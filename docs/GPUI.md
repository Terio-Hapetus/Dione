# GPUI Notes

## Why GPUI

Lightweight + GPU-accelerated (120 FPS target), small binaries (~12 MB
min), native feel. Linux renders via blade/Vulkan (lavapipe works headless
under Xvfb).

## Pins (do not change casually)

- `gpui = "=0.2.2"` from crates.io only — never mix with zed git deps
  (type conflicts).
- `gpui-component = "=0.5.1"` + `gpui-component-assets` (Label, Button,
  Input, TextView markdown, dock/resizable, tables).
- `longbridge/gpui-component` main tracks zed git — check compat before
  upgrading; 0.5.1 matches our pin.

## Community resources

- `longbridge/gpui-component` (13k+ stars): 60+ components, dock layout
  (resizable panels/tabs/splits), virtual tables/lists, code-editor stub
  (Tree-sitter + LSP hooks), markdown/HTML, charts, themes. AI skills:
  `npx skills add longbridge/gpui-component` (`gpui` + `gpui-component`).
- `gpui-ce/gpui-ce`: community-edition fork, API-compatible, stability focus.
- `edo-zhou/awesome-gpui`: curated crates/examples/tools.
- Upstream: Zed source (`crates/gpui`), Zed Discord, `gpui.rs`.

## API pitfalls (hit before — don't rediscover)

- Bootstrap: `Application::new().with_assets(..).run(|cx| {
  gpui_component::init(cx); cx.open_window(opts, |w, cx|
  cx.new(|cx| Root::new(AnyView::from(v), w, cx))) })`.
  Need `use gpui::AppContext as _` for `.new()` on `&mut App`.
- `overflow_y_scroll()` and `.on_click()` only on `.id(...)` (stateful)
  elements; `rgba(u32)` is NOT const — wrap in helper fns;
  no `.whitespace_pre()` on Div.
- `WeakEntity::read_with(cx, |app, cx_app| ...)` takes 2 args; `cx.listener`
  returns Fn so `.clone()` captures inside.
- `Label` at `gpui_component::label::`; Button has `.outline()`
  (no `.ghost()`); `TextView::markdown(id, md, window, cx)` needs a unique
  id per block; `InputState::new(window, cx)` + `InputEvent::PressEnter`;
  `Input::new(&entity_input_state)`.
- `use gpui::*` imports gpui's `test` macro: plain `#[test]` in the same
  file resolves to it and recurses (`recursion limit reached`). In test
  modules use explicit paths (`crate::views::x::{f}`), never `use super::*`.
- Headless smoke must unset Wayland vars or GPUI picks Wayland over Xvfb.
- System deps: build-essential, libxkbcommon(-x11), libwayland, X11 client
  libs, x11-utils (xwininfo), xvfb, mesa-vulkan-drivers, DejaVu fonts
  (GPUI text system needs a resolvable font).
