#!/usr/bin/env python3
"""Synthetic compiled-adapter peer; never starts a shell, GUI, or harness."""
import json
import os
import sys
from pathlib import Path

root = Path(os.environ["HOST_ROOT"])
mode = os.environ["HOST_MODE"]
args = sys.argv[1:]
if args == ["ping"]:
    (root / "pinged").touch()
    sys.exit(7 if mode == "unavailable" else 0)
if args == ["capabilities"]:
    print(json.dumps({"capabilities": [] if mode == "capability-failure" else [
        "workspace.groups.v1", "workspace.group_create.v1", "workspace.create_in_group.v1"
    ]}))
    sys.exit(0)

path = root / "state.json"
state = json.loads(path.read_text()) if path.exists() else {
    "workspaces": {"unrelated": {"id": "unrelated", "current_directory": "/synthetic/other"}},
    "groups": [], "closed": [],
}
reply = {}
if args[0] == "new-workspace":
    options = dict(zip(args[1::2], args[2::2]))
    assert options["--group"] == "group"
    assert options["--window"] == "window"
    assert options["--focus"] == "false"
    state["workspaces"]["task"] = {"id": "task", "current_directory": options["--cwd"]}
    state["groups"][0]["member_workspace_ids"].append("task")
    state["startup_command"] = options["--command"]
    state["title"] = options["--name"]
else:
    assert args[0] == "rpc", args
    method, params = args[1], json.loads(args[2])
    if (mode, method) in [
        ("inventory-failure", "workspace.list"),
        ("focus-failure", "workspace.select"),
        ("close-failure", "workspace.close"),
    ]:
        sys.exit(8)
    if method == "workspace.current":
        reply = {"window_id": "window"}
    elif method == "window.list":
        reply = {"windows": [{"id": "window"}]}
    elif method == "workspace.list":
        reply = {"workspaces": list(state["workspaces"].values())}
    elif method == "workspace.group.list":
        reply = {"groups": state["groups"]}
    elif method == "workspace.group.create":
        group = {"id": "group", "name": params["name"], "anchor_workspace_id": "anchor",
                 "member_workspace_ids": ["anchor"], "is_collapsed": False}
        state["groups"].append(group)
        state["workspaces"]["anchor"] = {"id": "anchor", "current_directory": params["cwd"]}
        reply = {"group": group}
    elif method == "workspace.select":
        assert params["workspace_id"] == "task"
        state["selected"] = "task"
    elif method == "workspace.close":
        assert params["workspace_id"] == "task"
        state["closed"].append("task")
        del state["workspaces"]["task"]
        state["groups"][0]["member_workspace_ids"].remove("task")
    else:
        raise AssertionError(method)
path.write_text(json.dumps(state))
if args[0] == "new-workspace" and mode == "creation-failure":
    sys.exit(9)
print(json.dumps(reply))
