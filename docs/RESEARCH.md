# Dione Research Notes

## Orca (onorca.dev, MIT, 60k+ stars)

Agent Development Environment: parallel agents × isolated worktrees in one
app. Takeaways: (1) fan-out + compare-diffs + merge-winner is the core
workflow; (2) inline diff notes sent straight back to the agent; (3) GPU
terminal with infinite splits; (4) per-worktree Chromium + Design Mode
(click element → inject into agent context); (5) BYOK, no lock-in.
Dione copies 1+2 in M2; 3 partially; 4 is post-MVP.

## OpenAI Codex App (macOS)

Thread/project/worktree model. Takeaways: (1) detached HEAD by default for
experiments, named branch to keep; (2) "Create branch here" vs "Hand off to
local" two paths; (3) 15 managed worktrees retention; (4) review queue with
inline diff; (5) `AGENTS.md` worktree notes (branch naming, per-worktree
install). Dione adopts 1, 3, 5 in M2.

## Hermes Agent (NousResearch, MIT, 240k+ stars)

Self-improving agent + `hermes -w` worktree isolation. Takeaways: (1) each
session gets own branch `hermes/<uuid>` + auto-cleanup + stale prune;
(2) `.worktreeinclude` copies gitignored files (`.env`, venvs);
(3) sidebar groups `parent repo → worktree → sessions`;
(4) kanban subtask = own worktree. Dione adopts 1–3 in M2.

## Warp (warp.dev)

Factories-as-code, fleet over SDLC, evals + self-improvement, governance.
Takeaway: post-MVP inspiration for `factory.yaml`-style config and
cost-per-PR tracking. Not in scope before M3.

## tldraw (tldraw.dev, 50k stars)

Infinite-canvas SDK for React (custom shapes, workflow/agent/chat starter
kits). Evaluated for the canvas direction — REJECTED for this repo
(canvas moved to a separate project); relevant only if Dione ever needs a
web-based canvas companion.
