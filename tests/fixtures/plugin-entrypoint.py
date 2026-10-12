#!/usr/bin/python3
"""Synthetic offline protocol peer; no network or authentication operations."""
import json
import os
import sys
import time

MODE = "normal"
MARKER = ""
with open(MARKER + ".tmp", "w") as marker:
    json.dump({"pid": os.getpid(), "cwd": os.getcwd(),
               "cwd_mode": os.stat(os.getcwd()).st_mode & 0o777}, marker)
os.replace(MARKER + ".tmp", MARKER)
assert sys.argv[1:] == ["--ahu-plugin-protocol=1"]
# Python and the macOS system interpreter shim add runtime/toolchain variables.
# None of these are ahu credentials or inherited host configuration.
assert set(os.environ) <= {"PATH", "LC_CTYPE", "CPATH", "LIBRARY_PATH",
                           "MANPATH", "SDKROOT", "__CF_USER_TEXT_ENCODING"}
if MODE in ("hang", "never_read"):
    time.sleep(30)
if MODE == "early_output":
    sys.stdout.write("x" * (65536 + 1))
    sys.stdout.flush()
request = json.load(sys.stdin)
assert set(request) == {"protocol_version", "plugin_id", "grant", "context", "input"}
assert request["protocol_version"] == 1
assert request["plugin_id"] == "fixture"
assert request["grant"] == {"operation": "invoke"}
if MODE == "stderr":
    sys.stderr.write("SECRET\x1b[31m" * 100000)
if MODE == "inherited_pipe" and os.fork() == 0:
    time.sleep(30)
    os._exit(0)
if MODE == "exact_limit":
    response = json.dumps({"status": "ok", "protocol_version": 1,
                           "result": {"input": request["input"], "context": request["context"]}})
    sys.stdout.write(response + " " * (65536 - len(response.encode())))
elif MODE == "mutation_result":
    json.dump({"status": "ok", "protocol_version": 1,
               "result": {"operation": "submit", "task": request["input"]["observations"][0]["snapshot"]["task"]}}, sys.stdout)
elif MODE == "overflow":
    sys.stdout.write("x" * (65536 + 1))
elif MODE == "malformed":
    sys.stdout.write("SECRET\x1b[31m")
elif MODE == "duplicate":
    sys.stdout.write('{"status":"ok","protocol_version":1,"result":{"key":1,"key":2}}')
elif MODE == "version":
    json.dump({"status": "ok", "protocol_version": 2, "result": None}, sys.stdout)
elif MODE == "extra":
    json.dump({"status": "ok", "protocol_version": 1, "result": None, "secret": "SECRET"}, sys.stdout)
elif MODE == "remote_error":
    json.dump({"status": "error", "protocol_version": 1, "code": "failed"}, sys.stdout)
elif MODE == "trailing":
    sys.stdout.write('{"status":"ok","protocol_version":1,"result":null} {}')
elif MODE == "partial":
    sys.stdout.write('{"status":"ok"')
else:
    json.dump({"status": "ok", "protocol_version": 1,
               "result": {"input": request["input"], "context": request["context"]}}, sys.stdout)
sys.stdout.flush()
if MODE == "hang_after_response":
    time.sleep(30)
if MODE == "nonzero":
    sys.exit(1)
