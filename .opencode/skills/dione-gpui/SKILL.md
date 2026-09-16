---
name: dione-gpui
description: GPUI 0.2.2 and gpui-component 0.5.1 API pitfalls for the ADE desktop UI
---

# ADE GPUI notes

Load this skill when touching `apps/desktop`.

## Pins

`gpui = "=0.2.2"` (crates.io only, never zed git),
`gpui-component = "=0.5.1"` + `gpui-component-assets`. Details: `docs/GPUI.md`.

## Bootstrap

```rust
use gpui::AppContext as _; // needed for `.new()` on `&mut App`
Application::new().with_assets(..).run(|cx| {
    gpui_component::init(cx);
    cx.open_window(opts, |window, cx| {
        cx.new(|cx| gpui_component::Root::new(AnyView::from(v), window, cx))
    })
});
```

## Pitfalls

- `overflow_y_scroll()` / `.on_click()` only on `.id(...)` elements.
- `rgba(u32)` is NOT const — wrap in helper fns. No `.whitespace_pre()`.
- `WeakEntity::read_with(cx, |app, cx_app| ...)` takes 2 args.
- `cx.listener(...)` returns Fn — `.clone()` captured vars inside.
- `Label` at `gpui_component::label::`; Button has `.outline()`, no `.ghost()`.
- `TextView::markdown(id, md, window, cx)` needs a unique id per block.
- `InputState::new(window, cx)` + builder `.auto_grow/.placeholder`;
  `InputEvent::PressEnter { .. }`; `Input::new(&entity)`.
- Snapshot polling: `cx.spawn` + 160 ms timer, `cx.notify()` only when the
  `Arc<Store>` ptr changes.
