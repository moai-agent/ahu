# Hermetic harness reference fixtures

These frozen Rust adapters are development fixtures for core protocol and lifecycle
regressions. They are not supported installation sources or release packages.
Production adapters live in independent ahu-plugin-Codex/Claude/Antigravity/OpenCode/Kiro
repositories (lowercase canonical names are listed in docs/harness-plugins.md).

For release conformance, build each production executable independently, put the
five binaries in an ignored validation directory and set AHU_TEST_HARNESS_BIN_DIR
to its canonical absolute path before running the core integration tests. The
normal test run uses these fixtures without fetching another repository.

Do not propagate provider behavior changes here automatically: first add a focused
core contract regression or update the corresponding production repository tests.
