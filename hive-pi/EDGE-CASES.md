# hive-pi Edge Cases and Bugs

Adversarial audit findings. Each section covers a bug category: severity, root cause, fix status, and the test that demonstrates it.

---

## Fixed Bugs

### BUG-1: Blocked tasks stuck permanently when a dependency fails

**Severity:** High — tasks become permanently unschedulable
**File:** `src/task-manager.ts:63`
**Status:** FIXED

**Root cause:** `unblockDependents()` was only called when a task reached a *success* terminal state (`Merged` or `Cancelled`). When a dependency reached `Failed`, the call was skipped entirely. The dependent task remained `Blocked` with a failed dep in its `blockedBy` array, with no mechanism to ever remove it.

The `unblockDependents` logic itself was correct — it already filtered remaining blockers to `!isTerminalTaskStatus` — but it was never given the chance to run.

**Fix:** Changed the `if` condition in `updateStatus` from:
```typescript
if (isTerminalTaskStatus(newStatus) && isSuccessTaskStatus(newStatus)) {
```
to:
```typescript
if (isTerminalTaskStatus(newStatus)) {
```

This ensures `unblockDependents` runs for all terminal transitions. When a dep fails, blocked dependents with no remaining non-terminal blockers are moved to `Pending` so the coordinator can decide whether to retry or cancel.

**Tests:** `test/edge-cases.test.ts` — "Bug 1: blocked tasks stuck when dependency fails"

---

### BUG-2: Worker without valid parent silently creates own worktree

**Severity:** Medium — workers operate in isolation from their lead
**File:** `src/hive.ts:91-97`
**Status:** FIXED

**Root cause:** `spawnAgent` for a Worker role used `params.parentAgent &&` before looking up the parent. If `parentAgent` was provided but didn't exist in `state.agents`, the lookup returned `undefined`, and the code fell through to the "Lead gets its own worktree" path. The worker silently created a new isolated worktree instead of sharing the lead's.

**Fix:** Added explicit validation:
1. Workers MUST provide a `parentAgent`.
2. The parent agent MUST exist in `state.agents`.
Both violations now throw a descriptive `Error`.

**Tests:** `test/edge-cases.test.ts` — "Bug 2: worker without valid parent gets own worktree"

---

### BUG-3: Partial JSONL line causes `SyntaxError` crash

**Severity:** Medium — process crash during message write corrupts recovery
**File:** `src/state.ts:106-111`
**Status:** FIXED

**Root cause:** `loadAgentMessages` split the file on `\n`, then called `JSON.parse()` on each line without any error handling. If the process was killed mid-write (OOM kill, power loss), the last line would be a partial JSON fragment. On the next process start, `loadAgentMessages` would throw `SyntaxError: Unterminated string` and crash.

**Fix:** Wrapped `JSON.parse` in a `try/catch` that silently drops unparseable lines. Valid messages are returned; corrupt lines are skipped. This makes message recovery resilient to partial writes.

**Tests:** `test/edge-cases.test.ts` — "Bug 3: partial JSONL line causes parse crash"

---

## Unfixed Issues (require architectural changes)

### BUG-4: Concurrent `processMergeQueue` TOCTOU race condition

**Severity:** High in multi-process environments; Low in current single-process usage
**File:** `src/hive.ts:223-251`
**Status:** DOCUMENTED — architectural fix needed

**Description:** `processMergeQueue` does a read-then-write with no locking:
1. Read: `state.mergeQueue.find(e => e.status === "pending")`
2. Write: `updateMergeQueueEntry(pending.taskId, { status: "merging" })`

If two processes both reach step 1 before either completes step 2, both see the same "pending" entry. Both will attempt to merge the same branch simultaneously, causing one to fail mid-merge and potentially leaving git in an inconsistent state.

**Secondary issue:** If the process crashes after step 2 (status set to "merging") but before the merge completes, the entry is permanently stuck as "merging". `processMergeQueue` only picks up "pending" entries, so a crashed "merging" entry is never retried.

**Proposed fix (not implemented):** Use a file-based advisory lock (e.g., `lockfile` or atomic `rename`-based mutex). Add a `merging_since` timestamp and a recovery mechanism that resets "merging" entries older than a timeout back to "pending".

**Tests:** `test/edge-cases.test.ts` — "Bug 4: concurrent merge queue processing" (tests single-process sequential guarantees; documents the stuck-merging scenario)

---

### BUG-5: `rebaseBranch` does not restore original HEAD

**Severity:** Low — current callers are not affected
**File:** `src/git.ts:117-132`
**Status:** DOCUMENTED

**Description:** `mergeBranch()` saves the current branch and restores it in the `finally` block. `rebaseBranch()` does NOT — after returning, HEAD is on `ontoBranch` regardless of where HEAD was before the call.

In `processMergeQueue`, `rebaseBranch` is called before a second `mergeBranch`. Since `mergeBranch` saves HEAD first, this works by accident. But any caller that expects HEAD to be preserved after `rebaseBranch` will be surprised.

**Proposed fix:** Add `const currentBranch = this.exec("git rev-parse --abbrev-ref HEAD")` at the start of `rebaseBranch` and restore it in the `finally` block, matching `mergeBranch`'s behavior.

---

### BUG-6: `isSuccessTaskStatus` includes `Cancelled` — silently unblocks dependents

**Severity:** Design ambiguity
**File:** `src/types.ts:16-18`
**Status:** DOCUMENTED — by-design but surprising

**Description:** `isSuccessTaskStatus(Cancelled) === true`. This means when a blocking task is cancelled (not completed), all of its dependents are unblocked and moved to `Pending`. A dependent task might then proceed to execute even though its declared dependency was never fulfilled.

This may be intentional (if a task is cancelled, remove the block so the coordinator can reschedule), but it is undocumented and surprising to anyone who assumes "Cancelled" means "the work was not done, so dependents should not proceed."

**Tests:** `test/edge-cases.test.ts` — "Bug 6: cancelling a dependency unblocks dependents (undocumented behavior)"

---

## Summary

| Bug | Severity | Fixed | Test |
|-----|----------|-------|------|
| BUG-1: Blocked tasks stuck on dep failure | High | ✅ | `test/edge-cases.test.ts` |
| BUG-2: Worker gets own worktree (missing parent) | Medium | ✅ | `test/edge-cases.test.ts` |
| BUG-3: Partial JSONL crashes on load | Medium | ✅ | `test/edge-cases.test.ts` |
| BUG-4: Concurrent merge queue TOCTOU | High | ❌ Needs arch fix | `test/edge-cases.test.ts` |
| BUG-5: `rebaseBranch` HEAD not restored | Low | ❌ Cosmetic fix | (no test) |
| BUG-6: Cancel silently unblocks dependents | Ambiguous | ❌ Design decision | `test/edge-cases.test.ts` |
