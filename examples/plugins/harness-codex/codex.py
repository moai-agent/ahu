#!/usr/bin/python3
"""Pure Codex launch translation, separate from Ahu's process supervision."""
import json
import sys

PROMPT = "<prompt: see prompt.txt>"


def unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate key")
        value[key] = item
    return value


def plan(request):
    fields = {"protocol_version", "harness", "mode", "model", "permissions", "work_mode", "session"}
    if not isinstance(request, dict) or set(request) != fields:
        raise ValueError("request fields")
    if type(request["protocol_version"]) is not int or request["protocol_version"] != 1 or request["harness"] != "codex":
        raise ValueError("identity/version")
    model, session = request["model"], request["session"]
    if not isinstance(model, str) or not model or model.startswith("-") or "\0" in model:
        raise ValueError("exact model required")
    if session is not None and (not isinstance(session, str) or not session or "\0" in session):
        raise ValueError("session")
    permissions = request["permissions"]
    if request["mode"] == "headless":
        if request["work_mode"] is not True or permissions != "auto":
            raise ValueError("only headless work mode is supported")
        args = ["exec"] + (["resume"] if session else [])
        args += ["--model", model, "--json", "-c", 'approval_policy="never"', "-c", 'sandbox_mode="danger-full-access"']
        if not session:
            args += ["--color", "never"]
        args += ["--"] + ([session] if session else [])
    elif request["mode"] == "interactive":
        if request["work_mode"] is not False or session is not None:
            raise ValueError("interactive state")
        args = ["-m", model]
        if permissions == "auto":
            args += ["--ask-for-approval", "never", "--sandbox", "workspace-write"]
        elif permissions == "accept-edits":
            args += ["--approve-for-me"]
        elif permissions != "prompt":
            raise ValueError("permissions")
        args += ["--"]
    else:
        raise ValueError("mode")
    args.append(PROMPT)
    return dict(protocol_version=1, harness="codex", program="codex", args=args, prompt_arg=len(args)-1)


def main():
    if sys.argv[1:] != ["--ahu-harness-launch-protocol=1"]:
        return 2
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            return 2
        request = json.loads(raw, object_pairs_hook=unique)
        result = json.dumps(plan(request), ensure_ascii=True, separators=(",", ":")) + "\n"
        if len(result.encode()) > 65536:
            return 2
        sys.stdout.write(result)
        return 0
    except (ValueError, TypeError, RecursionError):
        return 2


if __name__ == "__main__":
    sys.exit(main())
