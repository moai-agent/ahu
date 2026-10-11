# Structured agent contracts

A registered agent may declare `input_schema` and/or `output_schema` directly
in its OKF YAML frontmatter. Each value is a nested JSON Schema object or a
boolean schema. String paths are not supported. For example, add these fields
to a manifest's existing frontmatter:

```yaml
input_schema:
  $schema: https://json-schema.org/draft/2020-12/schema
  type: object
  required: [files]
  properties:
    files:
      type: array
      minItems: 1
      items:
        type: object
        required: [path]
        properties:
          path: {type: string, minLength: 1}
        additionalProperties: false
  additionalProperties: false
output_schema:
  type: object
  required: [ok, notes]
  properties:
    ok: {type: boolean}
    notes:
      type: array
      items: {type: string}
  additionalProperties: false
```

Schemas are validated when the manifest loads. Their values participate in the
agent identity and are frozen with a child grant and task attempt. Commit the
manifest and update `ahu.lock` as with other agent context changes.

For an input artifact containing `{"files":[{"path":"src/lib.rs"}]}`, dispatch
a granted child with an ordinary assignment and its structured input:

```sh
ahu @reviewer --headless --prompt 'Review the supplied files.' --input-file /absolute/path/outside/repositories/input.json
```

ahu validates the bounded input before broker dispatch, harness probing, or
worktree creation. It delivers the validated JSON in the child's request. The
child writes its JSON result, for example `{"ok":true,"notes":[]}`, to the
file named by `AHU_STRUCTURED_RESULT` before finishing. This location is specific
to the current attempt. Missing, malformed, oversized, or nonconforming output
prevents successful completion. Validated output is retained as
`structured_output` in `ahu result TASK --output json`, even after request-file
cleanup. Schema validity and process success do not replace review of the work.

Contracts require headless execution. An output contract refuses read-only
Codex and bounded-helper profiles because they cannot write the result; it does
not widen their permissions. Without schemas, prompt-only manifest parsing,
identity digests, and dispatch behavior remain unchanged. An input artifact
without an input schema is refused rather than silently ignored.

## Supported vocabulary and bounds

Schemas use JSON Schema Draft 2020-12. An absent `$schema` selects that draft;
an explicit value must be `https://json-schema.org/draft/2020-12/schema`.
Supported keywords are:

- Boolean schemas, `type` (including unions), `enum`, and `const`.
- `properties`, `required`, `additionalProperties`, `minProperties`, and `maxProperties`.
- `items`, `prefixItems`, `uniqueItems`, `minItems`, and `maxItems`.
- `minLength`, `maxLength`, `allOf`, `anyOf`, `oneOf`, and `not`.
- `title`, `description`, `default`, and `examples` as annotations only.

Other keywords and dialects are rejected. Unsupported features include `$ref`,
`$dynamicRef`, `$defs`, regex and format assertions, numeric range constraints,
conditionals, and dependent/unevaluated-property keywords. No local or remote
reference is fetched.

Each schema and artifact is limited to 64 KiB of JSON, depth 32, and 4096 nodes.
Schema checking and artifact evaluation each have a 100000-step budget. YAML
schema expansion also enforces node, nesting, and string-byte limits while
parsing aliases. Duplicate keys, custom YAML tags, merge keys, and non-string
mapping keys are rejected. Quote YAML strings that would otherwise become
booleans or null, such as `type: "null"` and property names like `"true"`.

YAML schema numbers must use integer scalar notation within signed or unsigned
128-bit range. Floating-point YAML scalars, including `1.0`, exponent notation,
and nonfinite values, are rejected: the YAML decoder would round them before
schema validation. JSON artifacts retain exact decimal numbers, with exponent
magnitude limited to 1000000. Length/count schema bounds use unsigned JSON
integers within 64-bit range.

Input/result artifacts must be singly linked regular files. Diagnostics name
fixed schema keywords and bounds without echoing private paths, values, broker
capabilities, or supervisor state. These checks do not isolate same-user
processes or establish semantic correctness.
