# Typed decisions over ahu MCP

This is an exploratory, model-neutral MCP interface for letting an ahu agent
ask a configured decision provider for bounded typed answers. The default
provider is TypeSafe Jev; an explicit local provider can still be selected for
offline experiments. ahu does not run an agent loop. The bundled
[`typed-decisions` skill](../.agents/skills/typed-decisions/SKILL.md) guides
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
between Jev's two-point min/max rubric back to the requested numeric range.
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
tool name, success/error outcome, question count,
unique question types, stable `telemetry_key` dimensions, service and model
identifiers, reported prompt/generated token counts, service timings, and a
fixed error category. A `telemetry_key` is an optional lowercase identifier
such as `department` or `refund_requested`; use stable, non-sensitive labels
from a small vocabulary. The local Ollama adapter removes these keys before
sending questions to the model; the Jev adapter does not send them upstream.
The span never records state, instructions, question
names, answer values, or error text. The resource carries the ahu agent,
harness, model, task ID, headless attempt, and optional `ahu.eval.run_id`,
`ahu.eval.case_id`, `ahu.eval.corpus_version`, and `ahu.eval.stage` identifiers
when ahu launched the session. This supports grouping across harnesses and
correlating MCP calls to the task result. MCP spans cover inbound requests;
they do not describe harness-internal tool use that bypasses ahu MCP. Directly
started MCP servers have no ahu task identity unless their launcher supplies
the corresponding allowlisted resource attributes. On normal server shutdown,
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

The MCP contract remains provider-neutral. ahu translates it to TypeSafe's
native API by default, or to the local service contract when
`AHU_DECISION_URL` is explicitly set. The included Ollama adapter remains an
optional local process; ahu itself does not manage model downloads, runtime
dependencies, device selection, or service lifecycle.

Tool calls are synchronous MCP calls; they are not recorded as ahu tasks. The
agent gets the service result as evidence and remains responsible for deciding
what to do. Probabilities are model estimates, not guarantees. Harnesses that
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
