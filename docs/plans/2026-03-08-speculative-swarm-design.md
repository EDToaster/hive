# Speculative Swarm: Hive v2 Design

## Vision

Transform Hive from a spec-execution pipeline into a **thinking partner** that explores problem spaces, challenges assumptions, and surfaces alternatives — making the human a curator of ideas rather than an architect of specs.

## Core Concepts

### Two-Phase Model: Explore then Execute

Hive gains a new primary mode alongside the existing execution pipeline.

**Explore phase** — lightweight, parallel, disposable:
- Human expresses vague intent ("make the dashboard faster")
- Coordinator converses with human to understand and refine intent
- 2-3 explorer agents prototype different approaches in parallel
- An evaluator agent compares results with metrics and qualitative analysis
- Human picks a direction

**Execute phase** — the current Hive model, triggered when needed:
- Small tasks: merge explorer output directly
- Medium tasks: spawn a single worker to polish the explorer's prototype
- Large tasks: full swarm execution using the explorer's prototype as reference spec

The key insight: **the human never writes a spec**. The spec emerges from the explore-phase conversation.

### Coordinator as Thinking Partner

The coordinator's role fundamentally changes. It operates in two modes:

**Think Mode** — conversational collaboration with the human:
- Analyzes the codebase and queries the Hive Mind
- Presents structured analysis: what it found, what the options are, what it recommends
- Goes back and forth with the human to refine direction
- Produces a proposal before any agents spawn

**Execute Mode** — orchestrating the swarm (current behavior, refined):
- Spawns explorers or full execution teams
- Manages lifecycle: explore, evaluate, decide, execute
- Processes merge queue, handles failures

The coordinator transitions fluidly between modes within a single run.

### The Promote Decision

After exploration, three outcomes:

1. **Merge directly** — explorer output is production-ready. Best for small tasks.
2. **Refine and merge** — mostly good, needs polish. Spawn a single worker on the explorer's branch.
3. **Full execution** — approach is proven but needs production-quality implementation. Seamlessly escalate to full swarm. The explorer's prototype becomes the reference, all Hive Mind discoveries carry forward, and the coordinator's proposal becomes the spec.

## New Agent Roles

### Explorer

Lightweight, one-shot agents for divergent exploration.

- Get a worktree + branch (`hive/<run-id>/explore-<n>`)
- Receive a specific mandate from the coordinator ("try Redis caching", "analyze why this approach fails")
- No sub-agents, no review cycle, no merge queue interaction
- Optional time/cost budget constraint
- Write discoveries to the Hive Mind as they work
- Branches are disposable by default — value is in knowledge, not just code
- Can be promoted to merge-ready if output is clean enough

Not every explorer writes code. Some are purely analytical — the **adversarial explorer** whose job is to argue against the proposed approach is often the most valuable one.

### Evaluator

Comparison agent spawned after all explorers complete.

- Gets read-only access to all explorer branches
- Runs tests, benchmarks, complexity metrics on each branch
- Produces a structured comparison document at `.hive/runs/<run-id>/evaluation.md`
- Qualitative analysis, not just numbers: "Approach A is simpler but doesn't handle cache invalidation"
- Coordinator presents this to the human with a recommendation

## The Hive Mind (Shared Knowledge Space)

A structured, evolving knowledge store at `.hive/mind/` that grows in real-time as agents work. Three layers:

### Layer 1: Discoveries (raw, agent-contributed)

Any agent can write a discovery during a run:
- Tagged with: source agent, confidence level, relevant file paths, timestamp
- Low barrier — agents note anything surprising or useful they find
- Stored as structured entries in `.hive/mind/discoveries.jsonl`

### Layer 2: Insights (synthesized, coordinator-curated)

The coordinator periodically synthesizes discoveries into higher-level insights:
- "Three agents independently noted tight coupling between auth and sessions — this is a systemic pattern"
- Insights link back to the source discoveries
- Stored in `.hive/mind/insights.jsonl`

### Layer 3: Conventions (persistent, cross-run)

Patterns validated across multiple runs. This layer **already exists** as `.hive/memory/conventions.md`. It remains the persistence/graduation layer — discoveries and insights feed into conventions at post-mortem time.

Conventions are injected into every agent's `CLAUDE.local.md` at spawn. The swarm starts every run smarter than the last.

### New MCP Tools

| Tool | Access | Description |
|---|---|---|
| `hive_discover` | Any agent | Write a discovery to the Hive Mind |
| `hive_query_mind` | Any agent | Search the knowledge space by topic/keyword |
| `hive_synthesize` | Coordinator | Promote discoveries into an insight |
| `hive_establish_convention` | Coordinator | Promote an insight into a convention (with human approval) |

### Relationship to Existing Memory System

The existing `.hive/memory/` system (operations.jsonl, conventions.md, failures.jsonl) is a post-run write-once archive. The Hive Mind is a live, real-time knowledge space. No duplication:

- Existing conventions.md becomes Layer 3 of the Hive Mind
- Existing operations/failures remain the post-mortem persistence layer
- Discoveries and insights are entirely new real-time layers
- Post-mortem agent graduates the best discoveries/insights into conventions

## Novel Interaction Patterns

### Cross-Pollination via Stigmergy

Agents coordinate indirectly through the Hive Mind, not direct messages. When one agent discovers something, all agents can find it via `hive_query_mind`. This creates lateral awareness without N-squared communication channels.

Example flow:
- Explorer-1 discovers "the API already returns cache headers but nothing consumes them"
- Explorer-2 queries the mind before starting, finds this, builds on it
- During execution, a frontend worker discovers a re-render issue, a backend worker reads this and adds ETags

Same mechanism ants use (stigmergy). Scales without requiring agents to know about each other.

### The Adversarial Explorer

Always spawn at least one explorer whose job is to argue against the proposed approach:
- Reads the codebase looking for reasons the approach fails
- Looks for: edge cases, performance cliffs, maintenance burden, hidden dependencies, conflicting abstractions
- Produces a structured critique, not code
- Discoveries go into the Hive Mind and inform the evaluator

Built-in devil's advocate that catches problems before they're built.

### Swarm Reflection

During longer execution runs, the coordinator periodically spawns a lightweight, read-only **reflection agent**:
- Reads all Hive Mind content and current task/agent statuses
- Looks for systemic patterns: agents struggling with the same module, overlapping work, shared failing assumptions
- Reports findings to the coordinator as a synthesis

Different from heartbeat/stall detection. Not "is this agent alive?" but "is the swarm collectively heading in the right direction?"

### Knowledge-Driven Spawning

Team composition adapts reactively to discoveries:
- Run starts with an initial team based on the proposal
- Agents discover cross-cutting concerns and write them to the Hive Mind
- Coordinator reads these, recognizes new work streams, and spawns additional leads/workers
- New agents start with full context from the Hive Mind — no rediscovery needed

The swarm adapts its own shape based on what it learns. The initial plan is a starting point, not a commitment.

## New CLI

```
hive explore "intent string"    # Launch exploration run
hive start "intent string"      # Existing: launch execution run (still works)
hive mind                       # Browse the Hive Mind contents
hive mind query "caching"       # Search the knowledge space
```

`hive explore` is the new primary entry point for most work. `hive start` remains for cases where the human already knows exactly what they want.

## End-to-End Flow

```
Human: hive explore "the dashboard feels slow"
  |
Coordinator [Think Mode]:
  - Reads codebase, queries Hive Mind
  - Presents analysis: "6 sequential API calls, no caching, 2.3MB bundle"
  - Proposes 3 exploration angles
  |
Human: "Explore all three, but bundle size is probably a red herring"
  |
Coordinator [Execute Mode]:
  - Spawns Explorer-1: parallelize + cache
  - Spawns Explorer-2: server-side aggregation
  - Spawns Explorer-3 (adversarial): measure before assuming
  |
Explorers work in parallel (~5-10 min):
  - Write discoveries to Hive Mind as they go
  - Explorer-3 finds N+1 query is the real bottleneck
  - Explorer-2 reads this discovery, pivots approach mid-work
  |
Evaluator compares:
  - Explorer-1: 1.9s page load, 47 lines, masks root cause
  - Explorer-2: 0.6s page load, 83 lines, fixes root cause
  - Recommends Explorer-2
  |
Coordinator [Think Mode]:
  - Presents comparison to human
  - "The adversarial explorer found an N+1 query. Explorer-2 fixed it. 6x improvement."
  |
Human: "Merge directly."
  |
Coordinator: promotes Explorer-2's branch to merge queue
  |
Done. ~15 minutes. Zero specs written.
```

## What Changes From Today

| Today | Speculative Swarm |
|---|---|
| Human writes detailed spec | Human expresses vague intent |
| Agents execute the plan | Agents explore the problem space first |
| Coordinator decomposes tasks | Coordinator has a conversation |
| Workers are siloed | Agents share discoveries via Hive Mind |
| No adversarial thinking | Devil's advocate is built in |
| Fixed team composition | Swarm shape adapts to discoveries |
| Spec-to-code pipeline | Intent-to-exploration-to-code pipeline |
| Human is architect | Human is curator |
