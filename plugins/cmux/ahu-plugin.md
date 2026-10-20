---
okf_version: "0.2"
type: ahu:plugin
title: cmux source package
description: Source package metadata for the compiled and executable cmux session host
tags: [session, terminal]
schema_version: 1
id: cmux-local
package: ahu-cmux-plugin
host_api: 1
cargo_manifest: Cargo.toml
---

# cmux source package

Source-only package metadata for local discovery. Select `cmux` to use the
compiled adapter. This source manifest does not declare a built executable;
see README.md for building and staging the ahu-cmux-host Host V1 server.
