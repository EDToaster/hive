Some small ideas:

[ ] Hotkey to callback to an editor or something
[ ] Verify better isolation. Do leads only see agent status from their descendents and ancestors?
[x] Explorers should be able to submit merge queue requests -- Now coordinators can merge and fallback to cherry picking
[x] Using different models for different agents to speed up. Haiku for evaluator and reviewer?

[ ] Rebase-based merging rather than merging-based merging. Or? Squash and merge?
[ ] Wait-for-activity can miss activities between calls

[x] Agent statuses should refresh on every MCP call. Sometimes, leads forget to check agent status, and assume it's not done
[x] Agent status returns unstaged changes and files. It should probably also return commits as well
[x] Newly spawned agents should base their worktree off of the worktree of the agent that spawned it. Right now, it spawns off of main
[x] Better support for feature branches. I think it always branches off main??
[x] wait_for_activity in the leads will get activity changes from other teams as well
[x] wait_for_activity should only return messages directed to that specific agent
[x] Explorers don't mark their task as complete
[x] Explorers can't get messages from coordinator: `Error: Coordinator can only send messages to leads.`

[x] Stop hook messages should advance read cursor

[x] When the worker finishes work and marks the task as complete and then exits, the lead checks the status of the workers too fast and still sees it working
[x] Read tool does not get logged
[x] File overwriting issues
      - output.jsonl is ovewritten every time even on respawn
      - .mcp.json gets overridden by hive :(
[x] Hive TUI - agent output:
      - The keybind text should be in the window border (top), not at the bottom
      - Session end message in the hive tui contains the last message that was sent. This is unnecessary as the lines above already contain the same message. 
      - By default, scroll to the bottom of the text. Make it clear that the text is at the bottom (allow a little bit of overscroll)
      - Add scroll bar
      - Activities -- for mcp tools that we own, add formatting and more rich information (WaitForActivity (timeout: 600))
      - Remove stall timer on coordinator
[x] Better error recovery. If an agent dies mid-stream, there is no way of resuming the session (since session ID is stored only at the end). Every json message from the claude instance is tagged with the session id, so it should be easy to get that information immediately.
[x] Can we stream output of the subagents for better visibility?
[x] Tool use reporting for anyone other than coordinator is broken again
[x] After the reviewer is spawned and returned with a verdict, the lead dies and nothing happens. The commits should land in the queue. The coordinator ends up having to manually use git commands to merge
