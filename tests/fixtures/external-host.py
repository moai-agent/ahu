#!/usr/bin/python3
"""Synthetic Host V1 peer: in-memory surfaces only, never starts a shell/terminal."""
import json
import os
import struct
import sys
import time

MODE = "normal"
MARKER = ""
PLUGIN_ID = "fixture"
STATE = ""

if MARKER:
    with open(MARKER, "w") as stream:
        json.dump({"pid": os.getpid(), "cwd": os.getcwd(),
                   "cwd_mode": os.stat(os.getcwd()).st_mode & 0o777,
                   "argv": sys.argv[1:],
                   "env": dict(os.environ)}, stream)

workspaces = {}
group = {"id": "group", "name": "fixture", "anchor_workspace_id": "anchor",
         "member_workspace_ids": ["anchor"], "is_collapsed": False}

if STATE and os.path.exists(STATE):
    with open(STATE) as stream:
        saved = json.load(stream)
        workspaces, group = saved["workspaces"], saved["group"]


def persist():
    if STATE:
        with open(STATE, "w") as stream:
            json.dump({"workspaces": workspaces, "group": group}, stream)


def read_exact(size):
    data = b""
    while len(data) < size:
        chunk = sys.stdin.buffer.read(size - len(data))
        if not chunk:
            sys.exit(0)
        data += chunk
    return data


def send(response):
    body = json.dumps(response, separators=(",", ":")).encode()
    frame = struct.pack(">I", len(body)) + body
    if MODE == "fragmented":
        for i in range(0, len(frame), 7):
            os.write(1, frame[i:i + 7])
    else:
        sys.stdout.buffer.write(frame)
        sys.stdout.buffer.flush()


while True:
    size = struct.unpack(">I", read_exact(4))[0]
    assert 0 < size <= 1024 * 1024
    request = json.loads(read_exact(size))
    assert request["protocol_version"] == 1
    method, params = request["method"], request["params"]
    response = {"protocol_version": 1, "id": request["id"], "result": None}
    if method == "hello":
        assert params["protocol_versions"] == [1]
        if MODE == "exit_hello":
            sys.exit(1)
        if MODE == "hang_hello":
            time.sleep(60)
        if MODE in ("oversize", "zero", "partial_header", "partial_body", "garbage"):
            if MODE == "oversize":
                os.write(1, struct.pack(">I", 1024 * 1024 + 1))
            elif MODE == "zero":
                os.write(1, struct.pack(">I", 0))
            elif MODE == "partial_header":
                os.write(1, b"\x00\x00")
            elif MODE == "partial_body":
                os.write(1, struct.pack(">I", 100) + b"{}")
            else:
                os.write(1, struct.pack(">I", 4) + b"nope")
            sys.exit(0)
        capabilities = params["required_capabilities"]
        response["result"] = {
            "protocol_version": 2 if MODE == "bad_negotiation" else 1,
            "host_api": 1,
            "plugin_id": "other" if MODE == "wrong_identity" else PLUGIN_ID,
            "capabilities": capabilities[:-1] if MODE == "missing_capability" else capabilities,
            "inherited_workspace": None,
        }
        if MODE == "duplicate_capability":
            response["result"]["capabilities"] += [capabilities[0]]
        if MODE == "wrong_id":
            response["id"] += 1
        if MODE == "wrong_version":
            response["protocol_version"] = 2
        if MODE == "unknown_envelope_field":
            response["extra"] = True
        if MODE == "both_result_error":
            response["error"] = {"code": "failed", "message": "synthetic"}
        if MODE == "stderr_flood":
            os.write(2, b"x" * (1024 * 1024))
    elif method in ("workspaces", "workspaces_in_window"):
        if MODE == "duplicate_workspace":
            body = (b'{"protocol_version":1,"id":' + str(request["id"]).encode()
                    + b',"result":{"complete":false,"complete":true,"workspaces":{}}}')
            sys.stdout.buffer.write(struct.pack(">I", len(body)) + body)
            sys.stdout.buffer.flush()
            sys.exit(0)
        response["result"] = {"complete": MODE != "incomplete_inventory", "workspaces": workspaces}
    elif method == "create_group":
        group["name"] = params["name"]
        workspaces["anchor"] = {"directory": params["cwd"], "title": params["name"]}
        response["result"] = group
    elif method == "list_groups":
        response["result"] = {"complete": True, "groups": [group]}
    elif method == "find_group":
        response["result"] = group if params["group_id"] == group["id"] else None
    elif method == "create_task_workspace":
        if MODE.startswith("durable"):
            import shlex
            words = shlex.split(params["startup_command"])
            assert words[1:3] == ["run-task", "--task-dir"]
            with open(os.path.join(words[3], "task.json")) as stream:
                task = json.load(stream)
                pin = task["session_host"]
                assert pin["package"] == "fixture-host"
                assert pin["executable"]["protocol_version"] == 1
                assert len(pin["executable_sha256"]) == 64
                assert task["cmux_workspace_id"] is None

        if MODE == "created_then_exit":
            with open(os.path.join(params["cwd"], "startup-observed"), "w") as stream:
                stream.write("synthetic startup side effect")
            sys.exit(0)
        if MODE == "exit_create":
            sys.exit(0)
        if MODE in ("hang_create", "cancel_create"):
            time.sleep(60)
        if MODE in ("not_created", "unknown_create", "failed_create"):
            del response["result"]
            response["error"] = {"code": {"not_created": "not_created", "unknown_create": "outcome_unknown", "failed_create": "failed"}[MODE], "message": "synthetic refusal"}
        else:
            # Models creation but deliberately never evaluates startup_command.
            workspaces["workspace"] = {"directory": "/wrong" if MODE == "wrong_cwd" else params["cwd"], "title": params["title"], "description": params["summary"]}
            group["member_workspace_ids"].append("workspace")
            response["result"] = {"workspace_id": "workspace", "window_id": "window", "group_id": "wrong" if MODE == "wrong_group" else "group"}
    elif method == "window_for_workspace":
        response["result"] = "wrong" if MODE == "wrong_window" else "window"
    elif method in ("current_workspace", "current_window"):
        response["result"] = None
    elif method == "close_workspace":
        workspaces.pop(params["workspace_id"], None)
        group["member_workspace_ids"].remove(params["workspace_id"])
    elif method in ("select_workspace", "set_status", "set_agent_metadata", "expand_group", "set_anchor", "add_workspace_to_group"):
        pass
    elif method == "create_anchor_workspace":
        response["result"] = "anchor"
    elif method == "open_markdown":
        response["result"] = "document"
    else:
        del response["result"]
        response["error"] = {"code": "failed", "message": "unsupported fixture operation"}
    if method == "create_task_workspace" and MODE == "malformed_create":
        response["result"] = "invalid result"
    persist()
    if method == "create_task_workspace" and MODE == "durable_uncertain":
        sys.exit(0)
    send(response)
    if method == "hello" and MODE == "backpressure":
        time.sleep(60)
