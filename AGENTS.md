# Project instructions for agents

## Commits and changelogs

Read [CHANGELOG.md](CHANGELOG.md) before preparing a commit. Follow its commit-message format: `+ Added`, `= Fixed`, and `- Removed`, with a concise subject and grouped body. Update the changelog alongside behavior changes, and report only validation that actually ran.

Review the exact staged files and diff before committing or pushing. Preserve unrelated work, exclude secrets, local app data, build output, and ignored private workspaces, and keep publication within the user's requested scope. Do not rewrite published history unless the user explicitly requests it.

## Desktop work

Use `agent-desktop` to see and operate desktop apps. Before first use, run `agent-desktop skills get desktop` and `agent-desktop skills get platform`, and follow them.

In Codex, use the configured agent-desktop MCP server for desktop actions and app discovery. Use its `desktop_list_apps` tool to enumerate apps because the Windows shell sandbox limits process visibility.
