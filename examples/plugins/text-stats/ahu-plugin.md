---
okf_version: "0.2"
type: ahu:plugin
title: Offline text statistics
schema_version: 1
id: text-stats
package: ahu-example-text-stats
host_api: 1
cargo_manifest: Cargo.toml
entrypoint:
  path: bin/text-stats
  protocol_version: 1
  timeout_ms: 5000
---

Accept exactly `{"text": "..."}` as input and return `characters` (Unicode code
points) and `words` (runs separated by Unicode whitespace or U+001C through U+001F).
Empty text has zero characters and words. No files, environment configuration,
network, credentials, or provider executables are used.
