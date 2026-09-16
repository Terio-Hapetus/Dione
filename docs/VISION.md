# Dione Vision

## What it is

Dione (Agentic IDE) is a native Linux desktop app for **running and inspecting
AI coding agents** — not for typing code yourself. The human directs and
judges; agents produce the code.

Core loop: give N agents N tasks → each works in its own isolated git
worktree → you review inline diffs → send notes back to the agent → merge
the winner.

## What it is NOT

- Not a code editor (noJetBrains/VS Code ambitions: no LSP editing, no
  debugging, no extensions marketplace).
- Not an infinite canvas (that lives in a separate project).
- Not a model harness (no memory routing, no eval loops, no guardrails
  engine). The agent engine is `opencode serve`, driven via `opencode-codes`.

## Who it serves

A solo developer running 2–5 coding agents in parallel on one repo, who today
hand-rolls the workflow with `git worktree` + tmux + browser PR tabs and
loses track of which agent needs attention.

## Influences (details in RESEARCH.md)

- **Orca**: parallel worktrees, annotate-diff-sent-back-to-agent, GPU
  terminal, BYOK. The closest product reference.
- **Codex App**: thread/project/worktree model, detached-HEAD experiments,
  handoff paths, 15-worktree retention.
- **Hermes Agent**: `hermes -w` worktree isolation, `.worktreeinclude`,
  `repo → worktree → sessions` sidebar grouping.
- **Warp**: factories-as-code and governance — post-MVP inspiration only.

## Success criteria

- M2: 3 agents run in parallel worktrees without file collisions; every
  agent completion is reviewable and mergeable from inside Dione.
- Review capacity (not typing speed) is the bottleneck the tool optimizes.
