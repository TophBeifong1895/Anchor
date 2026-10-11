"""The built-in Python dependency probe.

It is a Plugin MCP server written with nothing but the standard library and the
one package its manifest declares (`idna==3.6`), so a successful `version` tool
call is evidence that this process is running on the view its Plugin declared and
that the declared package came from the shared store.

It speaks MCP over stdio: one JSON-RPC message per line, which is what Anchor's
MCP host reads. Set `ANCHOR_PROBE_ADDRESS` to a `host:port` the caller is
listening on to also report whether this process can reach the network.
"""

import json
import os
import socket
import sys

import idna


def network_state():
    address = os.environ.get("ANCHOR_PROBE_ADDRESS")
    if not address:
        return "not_probed"
    host, port = address.rsplit(":", 1)
    try:
        socket.create_connection((host, int(port)), timeout=3)
        return "reachable"
    except OSError as error:
        return "denied: " + type(error).__name__


def report():
    return json.dumps({
        "version": idna.__version__,
        "file": idna.__file__,
        "prefix": sys.prefix,
        "exe": sys.executable,
        "network": network_state(),
    })


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request = json.loads(line)
        method = request.get("method")
        if method == "initialize":
            result = {
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "python-probe", "version": "1.0.0"},
            }
        elif method == "tools/list":
            result = {"tools": [{
                "name": "report",
                "description": "What this Plugin's view imports, and whether it can reach the network",
                "inputSchema": {"type": "object", "properties": {}},
            }]}
        elif method == "tools/call":
            result = {"content": [{"type": "text", "text": report()}]}
        else:
            # A notification, or a method this probe does not implement.
            continue
        send({"jsonrpc": "2.0", "id": request.get("id"), "result": result})


if __name__ == "__main__":
    main()
