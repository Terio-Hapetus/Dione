# ADE Architecture

## Crates

```
crates/
├── ade-core/          # runtime, headless-testable — NO UI dependency
│   ├── config.rs      # AppConfig (~/.config/ade/config.toml)
│   ├── worktree.rs    # pure helpers: slug/branch/path + WorktreeRecord
│   ├── state.rs       # facade → state/{types,store,events}.rs
│   ├── server.rs      # spawn/manage `opencode serve` + client
│   ├── runtime.rs     # facade → runtime/{commands,session,sse,
│   │                  #   reconcile,handlers,io}.rs
│   ├── transcript.rs  # UnifiedMessage/Cost/TaskId (shared leaf types)
│   └── context.rs     # context-window view-model + token est
├── ade-agent/         # AgentBackend trait + MockAgent + OpencodeAdapter
│                      # (moved from ade-core; depends one-way on ade-core)
├── ade-workspace/     # Task + WorkspaceProvider { HostProvider, Mock }
│                      # + PodmanProvider (exec, secrets-via-env) + ContainerManager
└── ade-ui/            # GPUI desktop bin
    └── src/{main,app}.rs + views/{theme,top_bar,sidebar,chat,
                                  composer,right_panel,diff,permission,
                                  vm_badge}.rs
```

## Data flow

UI thread → `Command` channel → runtime thread → mutate `Store` →
publish `Arc<Store>` via `RwLock` → GPUI view polls every 160 ms,
`cx.notify()` only when the Arc ptr changes.

## Store (ade-core/src/state.rs)

- `sessions: BTreeMap<id, Session>` / `statuses: BTreeMap<id, SessionStatus>`
- `messages: BTreeMap<sid, Vec<MessageEntry>>` (`MessageEntry { info, parts }`)
- `diffs`, `todos`, `pending_permissions`, `providers`, `selected_model`
- `totals: Totals { input, output, cache_read, cost }` (recomputed on set)
- `active_session`, `worktrees` (M0), `errors` (cap 20), `conn`
- `apply_event(&Event)`: SessionCreated/Updated/Deleted, SessionStatus,
  SessionIdle, MessageUpdated (upsert by id), MessagePartUpdated
  (replace-or-push part by id), PermissionAsked/Replied, TodoUpdated.
  Everything else ignored — REST reconcile is authoritative.

## Commands (runtime.rs)

`CreateSession { title }` · `SelectSession { id }` · `Prompt { text }` ·
`Abort` · `FetchDiff(sid)` · `PermissionReply { permission_id, response }` ·
`SetModel { provider_id, model_id }`.

## Runtime loop

- `spawn(config) -> RuntimeHandle` (own OS thread, tokio 2 workers).
- `outer_loop`: start server → `run_session` → on failure mark
  `Disconnected`, push error, retry in 5 s.
- `run_session`: bootstrap (sessions + providers) → spawn SSE pump →
  `select!` on commands vs poll tick (`poll_interval_ms`).
- SSE pump: `Connected` → reconcile active messages; `Event(e)` →
  `apply_event` (+ re-list sessions on create/delete/idle); errors logged.
- Poll tick: sessions + active messages + todos; providers every 10th tick.

## Wire endpoints used

- Wrapped: `POST /session`, `POST /session/{id}/prompt_async`,
  `GET /session/{id}/message`, `POST /session/{id}/abort`,
  `POST /session/{id}/permissions/{pid}` (deprecated upstream but works),
  `GET /event` (SSE).
- Raw via `client.request`: `GET /session`, `GET /session/{id}/diff`,
  `GET /session/{id}/todo`, `GET /provider` (defensive parse),
  `GET /global/health`.

## UI (ade-ui/src/app.rs + views/)

`AdeApp { rt, store, input, right_tab, model_ix, diff_notes,
annotate_target }` (state + actions + root `Render` only).
TopBar (conn dot, model picker, ctx/cost) · Sidebar (sessions, click to
select, + new) · Timeline (user right / agent left, markdown, tool cards,
`{}` inspect buttons) · Composer (Send/Abort) · Right panel
(Context/Inspector/Diff tabs) · Permission overlay · Error strip.
