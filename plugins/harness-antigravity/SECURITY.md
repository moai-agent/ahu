# Security

This package is an argument planner, not an execution sandbox. It accepts a bounded,
strict JSON request and emits a bounded plan. It does not launch native tools,
read configuration, access credentials, or perform network I/O. Its five-second
watchdog bounds stalled I/O; the caller must also enforce the manifest deadline.
Reject nonzero exit, partial output, identity mismatches and unvalidated plans.
Never treat a plan as permission to widen a task's frozen authority.

Native hooks, settings, trust, MCP, telemetry, events, usage accounting, and child
lifecycle remain outside this package. Only explicitly accepted native versions
are mapped. Mode support and live certification are distinct; see README.md.
Models and session IDs have bounded ASCII grammars. Diagnostics contain fixed
messages only. Do not add request bodies, native output, or configuration to
errors or telemetry. Keep real traces outside source repositories.

Report vulnerabilities privately to the package distributor or repository owner
through their verified private security contact. Do not post credentials, real
traces, or sensitive reproductions in public issues. No dedicated hosted advisory
endpoint or published package repository is asserted here. Use synthetic examples
when proposing public fixes and regression tests.
