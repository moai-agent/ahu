#!/usr/bin/python3
"""A self-contained, offline Ahu executable plugin using only protocol V1."""
import json
import sys


def respond(**fields):
    print(json.dumps({"protocol_version": 1, **fields}, separators=(",", ":")))


def main():
    if sys.argv[1:] != ["--ahu-plugin-protocol=1"]:
        respond(status="error", code="unsupported")
        return
    try:
        request = json.load(sys.stdin)
    except (ValueError, UnicodeError):
        respond(status="error", code="invalid_input")
        return
    if (
        not isinstance(request, dict)
        or set(request) != {"protocol_version", "plugin_id", "grant", "context", "input"}
        or request["protocol_version"] != 1
        or request["plugin_id"] != "text-stats"
        or request["grant"] != {"operation": "invoke"}
    ):
        respond(status="error", code="unsupported")
        return
    data = request["input"]
    if not isinstance(data, dict) or set(data) != {"text"} or not isinstance(data["text"], str):
        respond(status="error", code="invalid_input")
        return
    text = data["text"]
    respond(status="ok", result={"characters": len(text), "words": len(text.split())})


if __name__ == "__main__":
    main()
