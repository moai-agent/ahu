# Typed decisions over ahu MCP

This is an exploratory, model-neutral interface for letting any ahu-launched
MCP-capable agent ask a local decision service for bounded typed answers. ahu
does not load a model or run an agent loop. The service process and any
model-specific adapter run separately.

## MCP tool

`ahu mcp serve` advertises `ahu_typed_decide`. Set `AHU_DECISION_URL` in the
environment of that MCP server to a credential-free `http://` address using a
local IP literal such as `127.0.0.1` or `[::1]`.
The tool is available without the variable, but calls return a clear
configuration error. For example, a harness MCP entry can set the environment
while starting `ahu mcp serve`; the exact configuration format is harness
specific. ahu copies recognized project MCP configuration into task worktrees,
but does not author or install native harness settings.

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
an instruction. `state` can be text or a JSON object. Requests are limited to
64 KiB, responses to 1 MiB, and a call times out after 30 seconds. ahu refuses
non-loopback URLs and redirects, and bypasses configured HTTP proxies, so this
first experiment cannot silently send decision inputs to a remote host.

## Run the Ollama adapter

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

Configure the environment of each harness's `ahu mcp serve` process with:

```text
AHU_DECISION_URL=http://127.0.0.1:8001/v1/decisions
```

The MCP call returns a typed JSON value; for example, the selected option and
a numeric probability. Ollama's constrained output ensures the values match
the declared JSON types and ranges, but a generated probability is still the
model's estimate, not a calibrated confidence. The adapter reports local Ollama
token counts for comparing usage, latency, and answers across agent runs.

When project OpenTelemetry is enabled, the separately launched `ahu mcp serve`
process exports a span for each parsed MCP request, including initialize,
discovery, tool listing, tool calls, and task/subscription operations. Tool
calls use the `ahu.mcp.tool.call` span name. A typed decision span records the
tool name, success/error outcome, question count,
unique question types, stable `telemetry_key` dimensions, service and model
identifiers, reported prompt/generated token counts, service timings, and a
fixed error category. A `telemetry_key` is an optional lowercase identifier
such as `department` or `refund_requested`; use stable, non-sensitive labels
from a small vocabulary. The Ollama adapter removes these keys before sending
questions to the model. The span never records state, instructions, question
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

The request stays local: the adapter accepts only loopback Ollama URLs, disables
environment-configured HTTP proxies, binds its HTTP listener to loopback, and
rejects models that are not installed locally. Requests and decisions are not
logged.

## Service and model boundary

The HTTP contract belongs to the decision service, not to any particular model.
A provider adapter translates this request into its native API and translates
the result back to the `answers` shape. That keeps ahu and the MCP schema
independent of Ollama, Laya, Jev, or a future local model. The included Ollama
adapter is a separate local process; ahu itself does not manage model downloads,
runtime dependencies, device selection, or service lifecycle.

Tool calls are synchronous MCP calls; they are not recorded as ahu tasks. The
agent gets the service result as evidence and remains responsible for deciding
what to do. Probabilities are estimates and may not be calibrated. Harnesses that expose
ahu MCP can use the same tool contract, though each harness still needs its own
native MCP server declaration. Cross-harness usability must be checked against
the harness versions and configurations in use.
