---
name: mcp-diagnostics
description: "Diagnose MCP connectivity and capability-discovery failures using transport, initialization, tool-list, and call evidence. Use for locating a stopped handshake or separating a server advertisement from an attempted or successful tool invocation."
---

Distinguish connected transport, successful initialize, successful tools/list, and successful tools/call. If initialize succeeds but tools/list times out, record discovery-blocked and capture the list timeout plus server stderr; do not claim a tool call was attempted. If tools/list succeeds but a tool is absent, record capability-missing. Repairing queued work follows queue recovery policy.
