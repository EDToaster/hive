# Add Heartbeat Updates

## Problem

Agents register an initial heartbeat at spawn time but never update it afterward. This means `hive_check_agents` (which compares heartbeat timestamps against `stall_timeout_seconds`) will declare every agent stalled after 5 minutes, even if they're actively working.

## Requirements

### 1. MCP heartbeat tool

Add a `hive_heartbeat` MCP tool that agents call to update their heartbeat timestamp. Any agent role can call it. It updates the agent's `heartbeat` field on disk and returns a success confirmation.

### 2. Automatic heartbeat via hooks

When spawning an agent, configure a `PostToolUse` hook that calls `hive heartbeat` after every tool use (alongside the existing `hive log-tool` hook). This way agents heartbeat automatically without needing explicit instructions in their prompts.

### 3. CLI `hive heartbeat` command

Add a `hive heartbeat --run <run-id> --agent <agent-id>` CLI command that updates the agent's heartbeat timestamp on disk. This is what the hook calls.

### 4. `hive_check_agents` improvements

The existing `hive_check_agents` MCP tool should:
- Compare each agent's heartbeat against the configured `stall_timeout_seconds`.
- Report agents as `stalled` if their heartbeat is older than the threshold.
- Report agents as `dead` if their PID is no longer alive.
- Return a structured summary: agent ID, role, status, last heartbeat age in seconds, and whether the process is alive.

## Out of Scope

- Auto-recovery of stalled agents (just detection for now).
- Changing the heartbeat storage format (keep it in the agent JSON file).
