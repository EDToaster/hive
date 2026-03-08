# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Hive

Hive is a stateless CLI + MCP server (Rust) that orchestrates a swarm of Claude Code agents for autonomous software development. It manages agent lifecycles, task state, message passing, git worktree isolation, and a sequential merge queue. All state lives in `.hive/` as JSON files + SQLite. No daemon — every invocation reads/writes `.hive/` directly.

## Build Commands

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo test --all-targets       # run all tests
cargo test <test_name>         # run a single test
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt --all -- --check     # check formatting
cargo fmt --all                # auto-format
```

## Architecture

Single binary, three entry points:
- `hive <command>` — CLI for humans
- `hive mcp --run <id> --agent <id>` — stdio MCP server for Claude Code agents
- `hive tui` — read-only monitoring dashboard

### Module Layout

- `src/types.rs` — shared data types: Agent, Task, Message, MergeQueue, enums for status/role/urgency
- `src/state.rs` — all `.hive/` filesystem reads/writes. Nothing else touches disk directly.
- `src/git.rs` — shells out to `git` CLI for worktree, merge, branch operations
- `src/agent.rs` — agent spawn sequence: worktree creation, config file generation (hooks.json, settings.local.json, CLAUDE.local.md), Claude Code process launch
- `src/cli.rs` — clap command definitions
- `src/mcp.rs` — MCP server via `rmcp` with `#[tool_router]` macro. All hive tools defined here.
- `src/logging.rs` — SQLite logging layer (`log.db`) for tool call tracking
- `src/tui.rs` — ratatui dashboard
- `src/main.rs` — CLI dispatch, wires commands to modules

### Key Design Decisions

- **Stateless**: hive reads `.hive/` from disk on every call. No in-memory state between invocations.
- **Agent hierarchy**: Coordinator (no worktree, strategic) → Leads (worktree, tactical) → Workers (worktree, implementation). Permissions enforced in MCP layer.
- **Message injection**: messages are delivered directly into agent Claude Code conversations, not polled.
- **Agent config via CLAUDE.local.md**: additive to any existing project CLAUDE.md, not checked into git. Generated per-agent with role, task, and constraints baked in.
- **MCP as primary interface**: agents interact with hive via MCP tools (structured params/responses). CLI is a thin wrapper for humans.
- **Git worktrees for isolation**: each lead/worker gets its own worktree + branch. Coordinator does not get a worktree.
- **Sequential merge queue**: leads submit approved branches, coordinator merges one at a time. Conflicts trigger auto-resolution or rebase-and-resubmit.

### Role Permissions

| Capability           | Coordinator | Lead              | Worker                    |
|----------------------|-------------|-------------------|---------------------------|
| Spawn agents         | leads only  | workers only      | no                        |
| Submit to merge queue| process only| yes               | no                        |
| Send messages        | to leads    | to workers + coord| to own lead only          |

## Dependencies

Key crates: `clap` (CLI), `rmcp` + `schemars` (MCP server), `ratatui` + `crossterm` (TUI), `rusqlite` (SQLite), `tokio` (async), `serde` + `serde_json` (serialization), `chrono` (timestamps), `uuid` (IDs), `notify` (file watching). Git operations shell out to `git` CLI.

## Implementation Plans

Detailed phase-by-phase plans are in `docs/plans/phases/`. The design spec is at `docs/plans/2026-03-08-hive-spec.md`.
