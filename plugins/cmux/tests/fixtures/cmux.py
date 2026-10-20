#!/usr/bin/env python3
"""Deterministic native CLI peer; never opens a GUI or executes startup commands."""
import json
import os
import sys
from pathlib import Path

root = Path(os.environ["FAKE_ROOT"])
mode = os.environ["FAKE_MODE"]
args = sys.argv[1:]
(root / "connected").touch()
if args == ["ping"]:
    print("native ping diagnostic")
    sys.exit(7 if mode == "ping-failure" else 0)
if args == ["capabilities"]:
    print(json.dumps({"capabilities": [] if mode == "missing-capabilities" else [
        "workspace.groups.v1", "workspace.group_create.v1", "workspace.create_in_group.v1"
    ]}))
    sys.exit(0)

(root / "dispatched").touch()
created = root / "created"
group = {"id": "group", "name": "repo", "anchor_workspace_id": "anchor",
         "member_workspace_ids": ["anchor", "existing"] + (["task"] if created.exists() else []),
         "is_collapsed": False}
if args[0] == "new-workspace":
    options = dict(zip(args[1::2], args[2::2]))
    assert options == {"--name": "title", "--description": "summary", "--cwd": "/synthetic/task",
                       "--command": "literal $(not-run); 'quoted'", "--group": "group",
                       "--group-placement": "end", "--focus": "false", "--window": "window"}, options
    created.touch()
    print("native creation diagnostic")
    if mode == "command-failure":
        print("failure " + "界" * 2000, file=sys.stderr)
        sys.exit(8)
    sys.exit(0)
if args[0] == "set-status":
    assert args in [["set-status", "ahu.task", "running", "--workspace", "task"],
                    ["set-status", "ahu.agent", "agent · harness · model", "--workspace", "task"]]
    print("native status diagnostic")
    sys.exit(0)
assert args[0] == "rpc", args
method, params = args[1], json.loads(args[2])
reply = {}
if method == "workspace.group.list":
    assert params in [{}, {"window_id": "window"}]
    if mode == "preflight-failure" or (created.exists() and mode == "postflight-failure"):
        sys.exit(9)
    if mode == "spawn-failure":
        Path(sys.argv[0]).unlink()
    reply = {"groups": [group]}
elif method == "workspace.current":
    assert params == {}
    reply = {"window_id": "window", "workspace_id": "existing"}
elif method == "system.identify":
    assert params["caller"]["workspace_id"] in ["existing", "task"]
    reply = {"caller": {"window_id": "window"}}
elif method == "workspace.group.add":
    assert params == {"group_id": "group", "workspace_id": "existing", "window_id": "window"}
elif method == "workspace.create":
    assert params == {"title": "anchor title", "cwd": "/synthetic/repo", "group_id": "group",
                      "group_placement": "top", "focus": False}
    reply = {"workspace_id": "anchor"}
elif method == "workspace.group.set_anchor":
    assert params == {"group_id": "group", "workspace_id": "anchor"}
elif method == "workspace.group.create":
    assert params == {"name": "repo", "cwd": "/synthetic/repo", "window_id": "window"}
    reply = {"group": group}
elif method == "workspace.group.expand":
    assert params == {"group_id": "group"}
elif method == "markdown.open":
    assert params == {"path": "/synthetic/read me.md", "focus": True}
    reply = {"surface_id": "surface"}
elif method in ["workspace.select", "workspace.close"]:
    assert params == {"workspace_id": "task"}
    if method == "workspace.close":
        (root / "closed").touch()
elif method == "window.list":
    assert params == {}
    reply = {"windows": [{"id": "window"}, {"id": "second-window"}]}
elif method == "workspace.list":
    if mode == "inventory-failure" and params["window_id"] == "second-window":
        sys.exit(10)
    if params["window_id"] == "second-window":
        reply = {"workspaces": [{"id": "other", "current_directory": "/synthetic/other"}]}
    else:
        assert params == {"window_id": "window"}
        reply = {"workspaces": [{"id": "task", "current_directory": "/synthetic/task",
                                 "custom_title": "title", "description": "summary"}]}
    if mode == "oversized-result":
        reply["workspaces"][0]["custom_title"] = "x" * (1024 * 1024)
else:
    raise AssertionError(method)
print(json.dumps(reply))
