# Example MCP-only harness

A package with no executable. Core answers `probe` (runs the declared program with
`--version` once) and `launch` (a direct session with no arguments). Models,
admission, setup, health checks and event decoding need a real harness plugin.
Any harness that supports MCP can still use ahu's agents, tasks and skills through
`ahu mcp serve`. See [harness plugins](../../docs/harness-plugins.md).
