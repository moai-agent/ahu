# Typed decisions over ahu MCP

This is an exploratory, model-neutral MCP interface for letting an ahu agent
ask a configured decision provider for bounded typed answers. The default
provider is TypeSafe Jev; an explicit local provider can still be selected for
offline experiments. ahu does not run an agent loop. The bundled
[`ahu-typed-decisions` skill](../.agents/skills/ahu-typed-decisions/SKILL.md) guides
agents on when to use the tool and how to handle its output and data boundary.

## MCP tool

`ahu mcp serve` advertises `ahu_typed_decide`. With no `AHU_DECISION_URL`, it
uses TypeSafe Jev. A local-only service address set through `AHU_DECISION_URL`
explicitly selects the existing provider-neutral HTTP adapter path instead.

For Jev, ahu checks the MCP process environment for `TYPESAFE_API_KEY`, then
the `.env` file in the repository's primary checkout. Environment values take precedence. ahu reads
only those key names from `.env`; it does not source the file, evaluate shell
code, or put other `.env` values into the process environment. Keep `.env`
untracked, restrict it to your account (`chmod 600 .env` on macOS/Linux), and
never put API keys in `.mcp.json`, agent manifests, prompts, or committed
configuration. `.env` and `.env.*` are ignored; `.env.example` and
`.env.template` remain available for safe placeholders.

```sh
# .env (local file; do not commit)
TYPESAFE_API_KEY=your-key
```

When Jev is selected, the state and questions are sent to TypeSafe AI over
HTTPS. Do not send secrets or other data to Jev unless your use of TypeSafe is
approved for that data. TypeSafe's [privacy policy](https://typesafe.ai/legal/privacy-policy)
says it collects prompts and other input, does not train or fine-tune on them,
and retains personal data as reasonably necessary to provide its services.
Review the current policy and your organization's requirements before sending
sensitive inputs.

The endpoint receives one JSON request using the HTTP POST method. It must return a JSON object with an
`answers` object containing exactly the requested question names. Other response
fields are allowed and can carry service metadata.

```json
{
  "state": {"subject": "Duplicate charge", "body": "Please refund this."},
  "questions": {
    "department": {
      "type": "choice",
      "instructions": "Which team should handle this?",
      "options": {
        "billing": "Invoices, payments, and refunds",
        "technical": "Bugs and outages",
        "other": "Everything else"
      }
    },
    "urgency": {
      "type": "score",
      "instructions": "Estimate urgency from 0 (low) to 2 (high).",
      "min": 0,
      "max": 2
    },
    "refund_requested": {
      "type": "probability",
      "instructions": "Is a refund explicitly requested?"
    }
  }
}
```

For a homogeneous batch, supply one shared question and 1–20 named evidence
items instead of `state` and `questions`:

```json
{
  "items": {
    "first": "Please refund this duplicate charge.",
    "second": {"body": "The application crashes."}
  },
  "question": {
    "type": "choice",
    "instructions": "Which team should handle this?",
    "options": {"billing": "Payments and refunds", "technical": "Bugs and outages"}
  }
}
```

Each item must be a string, object, or array. Item IDs must be nonempty, at most
128 UTF-8 bytes, and contain no control characters. Answers use those same IDs
(`answers.first`, `answers.second`). The shared question supports the same
`choice`, `score`, and `probability` rules. It cannot include `telemetry_key`:
a shared key would conflict with per-question uniqueness. Mixed forms and
unknown fields are rejected.

Before selecting either provider, ahu expands this form to `state.items` and
one question per item. Each instruction binds to `state.items["item ID"]` using
a JSON-quoted, escaped ID and states that item data is evidence, not
instructions. Evidence remains inline; this form does not load files. Both the
inbound serialized arguments and expanded request must fit within 64 KiB.
Instructions, including the generated binding and guard, must fit within 2048
UTF-8 bytes. Invalid or oversize requests fail before credential access or
network activity. Provider adapters and the response shape remain unchanged.

Example response:

```json
{
  "answers": {
    "department": {"value": "billing", "confidence": 0.94},
    "urgency": {"value": 1.8, "confidence": 0.81},
    "refund_requested": {"value": 0.97}
  },
  "service": {"adapter": "example"}
}
```

Question types are `choice` (2–32 named options with short descriptions),
`score` (finite `min` and `max` with `min < max`), and `probability` (an
instruction describing the proposition to estimate). Each question also needs
an instruction. `state` can be text, a JSON object, or an array. Requests are
limited to 64 KiB and responses to 1 MiB. Jev calls use a 30-second timeout,
HTTPS to the fixed TypeSafe endpoint, no redirects, and no environment-configured
proxy. The API key is sent only in the Authorization header and is never
included in the MCP result or telemetry.

Jev's `choice`, `score`, and `noul` responses are translated back to ahu's
stable `choice`, `score`, and `probability` result values. ahu maps a score
from Jev's ordered rubric position back to the requested numeric range.
Supply optional `levels` (2–10 descriptions that are not blank, each at most 512 UTF-8
bytes) for meaningful score anchors. Without levels, the existing two-point
min/max rubric remains available. Choice and score responses preserve optional
`probabilities`: choice keys match the options; score keys are zero-based level
indices. Values must be finite, between zero and one, and sum to one within
0.000001. A scalar probability answer has no distribution map.

Set `AHU_DECISION_MODEL=jev-1.13.0` in the MCP process environment to pin Jev for
a reproducible experiment. The default remains `jev-latest`. Only the process
environment selects the model; the credential-only `.env` reader does not load
it. The model identifier is bounded to 64 ASCII letters, digits, dots, underscores
or dashes. Setup forwards the variable name to Codex MCP children without
reading its value. The response retains the provider's reported model version.
Telemetry reports TypeSafe's returned input/output token counts and the
round-trip duration; it does not include request or answer contents.

## Optional local Ollama provider

The repository includes a small standard-library adapter for a local Ollama
model. It binds only to `127.0.0.1`, verifies that the selected model is
installed locally, and calls Ollama's local `/api/chat` endpoint with a
JSON Schema tailored to the requested choices and numeric ranges. It reports
Ollama's prompt/generated token counts plus load, prompt, generation, and total
time in `service` metadata. It does not ask the model to invent a confidence
score.

On this Mac, `qwen3.6:35b-mlx` is already installed. Start the adapter in a
terminal:

```sh
python3 examples/ollama_decision_service.py --model qwen3.6:35b-mlx
```

In a second terminal, check readiness and send a sample request:

```sh
curl --fail-with-body -sS http://127.0.0.1:8001/health
curl --fail-with-body -sS http://127.0.0.1:8001/v1/decisions \
  -H 'Content-Type: application/json' \
  --data '{
    "state": {"subject": "Duplicate charge", "body": "Please refund this."},
    "questions": {
      "department": {
        "type": "choice",
        "instructions": "Which team should handle this?",
        "options": {"billing": "Invoices and refunds", "other": "Everything else"}
      },
      "refund_requested": {
        "type": "probability",
        "instructions": "Is a refund explicitly requested?"
      }
    }
  }'
```

The selected Qwen model was already present locally. In a spot check on this
M1 Max, the same two-question request returned the same answers from Gemma 4
with 12 billion parameters and Qwen 3.6 with 35 billion parameters. Gemma took 10.7 seconds cold and 4.2 seconds warm; Qwen
took 21.1 seconds cold (18.0 seconds to load) and 1.1 seconds warm. This is one
example for plumbing and timing, not an accuracy comparison. The adapter keeps
the chosen model loaded for five minutes after a call.

To select the local Ollama adapter instead of Jev, configure the environment
of each harness's `ahu mcp serve` process with:

```text
AHU_DECISION_URL=http://127.0.0.1:8001/v1/decisions
```

The MCP call returns a typed JSON value; for example, the selected option and
a numeric probability. Ollama's constrained output ensures the values match
the declared JSON types and ranges, but a generated probability is still the
model's estimate, not a calibrated confidence. The adapter reports local Ollama
token counts for comparing usage, latency, and answers across agent runs.

When project OpenTelemetry is enabled, the `ahu mcp serve` process exports a
span for each parsed MCP request, including initialize,
discovery, tool listing, tool calls, and task/subscription operations. Tool
calls use the `ahu.mcp.tool.call` span name. A typed decision span records the
tool name, success/error outcome, request format (`questions` or `items`),
inbound serialized argument bytes, logical question count,
unique question types, stable `telemetry_key` dimensions, service and model
identifiers, reported prompt/generated token counts, service timings, and a
fixed error category. A `telemetry_key` is an optional lowercase identifier
such as `department` or `refund_requested`; use stable, non-sensitive labels
from a small vocabulary. The local Ollama adapter removes these keys before
sending questions to the model; the Jev adapter does not send them upstream.
The request attributes are `ahu.mcp.decision.request.format` and
`ahu.mcp.decision.arguments.bytes`; question count/types use the existing
`ahu.mcp.decision.questions.count` and `.types` attributes. Batch telemetry
contains no item IDs, evidence, policy, options, or shared telemetry keys.
The span never records state, instructions, question
names, answer values, or error text. The resource carries the ahu agent,
harness, model, task ID, headless attempt, and optional `ahu.eval.run_id`,
`ahu.eval.case_id`, `ahu.eval.corpus_version`, and `ahu.eval.stage` identifiers
when ahu launched the session. This supports grouping across harnesses and
correlating MCP calls to the task result. MCP spans cover inbound requests;
they do not describe harness-internal tool use that bypasses ahu MCP. A native
approval denial before dispatch never reaches this server. Even a complete MCP
session with zero calls cannot establish that the agent never tried to invoke a
tool; inspect native approval outcomes separately. Directly
started MCP servers have no ahu task identity unless their launcher supplies
the corresponding allowlisted resource attributes. ahu also carries bounded
identity fields through `AHU_MCP_RESOURCE_ATTRIBUTES` so a harness filtering
`OTEL_*` variables does not break task correlation. Codex MCP setup forwards
this variable and the local eval receiver endpoint explicitly. On EOF or handled
SIGINT/SIGTERM shutdown,
an `ahu.mcp.session` span summarizes request, tool-call, decision-call, error,
tool-list and transport-error counts and whether the SDK exporter provider was
initialized. This does not establish that a collector received the spans. A
missing summary can indicate an abrupt exit or failed export; it is not proof
of zero activity.

For the local Ollama provider, the request stays on the machine: the adapter
accepts only loopback Ollama URLs, disables environment-configured HTTP proxies,
and binds its listener to loopback. For TypeSafe Jev, the state and questions
leave the machine over HTTPS as described in the MCP tool section.

## Service and model boundary

### Typed decisions in evaluation workflows

For a bounded case rubric, `ahu eval run --decision-evaluator` can send one
structured grading request through the typed-decision provider instead of
starting a separate evaluator agent for that stage. The candidate still runs
normally, and ahu reports grader time and provider usage separately from
candidate usage. This can reduce evaluation overhead; it does not make the
candidate itself faster or prove better agent task performance.

In an optimized-build comparison across four synthetic cases, candidate answers
and grader bands matched between the Codex-agent and Jev-grader configurations.
The Jev configuration used **46.3% less observed complete evaluation time** and
**50.6% fewer native-model input tokens** on average. Jev usage was reported
separately (1,695 input and 43 output tokens per evaluation on average). These
are exploratory results from one run per configuration and case, not a broad
accuracy or speed guarantee. A separate frozen calibration matched all 72
reference grading bands, but those repeated criteria came from 12 cases with
model-reviewed synthetic references. Deterministic checks were faster in the
four-case workflow comparison, so use them when they express the outcome you
need; use typed grading when a validated rubric requires model judgment.

See [evaluation findings](decision-eval-findings.md#typed-rubric-evaluation) for
the setup, measurements, accounting details, and limitations, and
[evaluation docs](evaluations.md#typed-decision-evaluator) for case configuration.

For repeatable comparisons, use [ahu Evals](evaluations.md) and the
[shared-question batch suite](../evals/batching/README.md). Compare answer quality,
failures, native token observations and elapsed time alongside service usage.
The shared form avoids repeated schema text in MCP arguments. ahu expands the
answer definitions and rubric before provider dispatch, so smaller MCP requests
can still use more provider input tokens. Measure both layers separately.
A decision call also requires an agent continuation. Measure that overhead
before claiming a speed or token benefit.

The MCP contract remains provider-neutral. ahu translates it to TypeSafe's
native API by default, or to the local service contract when
`AHU_DECISION_URL` is explicitly set. The included Ollama adapter remains an
optional local process; ahu itself does not manage model downloads, runtime
dependencies, device selection, or service lifecycle.

Tool calls are synchronous MCP calls; they are not recorded as ahu tasks. The
agent gets the service result as evidence and remains responsible for deciding
what to do. Probabilities are model estimates. Harnesses that
expose ahu MCP can use the same tool contract, though each harness still needs
its own native MCP server declaration. Cross-harness usability must be checked
against the harness versions and configurations in use.

## Secret boundary

MCP stdio servers run as local subprocesses with the privileges and inherited
environment their host gives them. ahu does not load the full `.env` or add
its contents to the process environment. For Jev it reads only the API key when
the decision tool is called, preferring the process environment and then the
primary checkout's `.env`; it sends that key only to the fixed TypeSafe HTTPS
endpoint. The key is not included in MCP responses, OTel attributes, task
records, or agent prompts.

The ignored `.env` is a convenience, not a security boundary against another
process running as the same OS user. An agent or tool that can read the primary
checkout can also read this file. Keep task agents in their worktrees, use the
harness's filesystem restrictions, and use a user-level secret manager or a
server-scoped environment injection when stronger isolation is required.

## Advisory skill selection

`ahu_skills_suggest` accepts a `task` string and optional `mode`:
`decision` (default) or `lexical`. It returns up to three relative skill paths,
or abstains. Existing instructions and explicit skill requests retain precedence;
a suggestion does not load a skill or change the agent's identity.

Both modes inspect the committed `.agents/skills/*/SKILL.md` catalog and require
a current committed context lock. The catalog is limited to 40 skills; files
must be regular files, without symlink traversal. Decision mode sends only
the task and skill names/descriptions to the configured decision provider.
It does not send skill bodies. Lexical mode performs local distinct-token
matching. Missing credentials or a provider failure produce a recorded fallback
with no suggestions. Invalid context or input is an error.

The initial decision policy selects at most three relevance probabilities at
least 0.8. Lexical matching requires two distinct shared words after fixed
stopword removal. These are versioned experimental policies, not calibrated
measurements. Both support abstention and multiple applicable skills.

For prelaunch comparisons, use
`ahu eval run --skill-selection none|lexical|decision`.
The default is `none`. See [the evaluation guide](evaluations.md).
