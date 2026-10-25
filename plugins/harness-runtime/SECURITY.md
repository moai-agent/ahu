# Harness runtime security

This library is linked only into installed external harness executables. Core
links the data-only protocol crate. Installing code is a trust decision: runtime
plugins run as the current user, and neither protocol bounds nor Git worktrees
confine filesystem or process access.

Discovery reads verified package metadata. Runtime operations require an explicit
`harness-runtime` grant. Core validates the declared executable, package binding,
framing, operation identity and launch plan. Native subprocess probes are bounded;
native authentication observations exclude credentials. Setup proposes changes
for core's conflict-checked transaction. Trust changes retain task-specific
receipts for cleanup and must not be blindly replayed after uncertain outcomes.

Decoder state is per attempt. Native frames and final summaries remain in memory;
never persist complete snapshots or include them in diagnostics. The transport
limits frames and deadlines. Core alone decides task acceptance and must reject
incomplete decoding. A valid provider response is evidence, not proof of tool
execution, instruction adherence, isolation or billing accuracy.
