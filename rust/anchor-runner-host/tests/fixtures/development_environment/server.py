import importlib.util
import json
import pathlib
import sys

# Formal and candidate tools must not borrow the private editable environment.
assert sys.prefix != "/development/env"
assert importlib.util.find_spec("editableprobe") is None
assert not pathlib.Path("/development/env").exists()

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "independent-env-probe", "version": "1.0"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "probe",
            "description": "Verify an independent formal Python context",
            "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]},
        }]}
    elif method == "tools/call":
        value = request["params"]["arguments"]["value"]
        observation = {"value": value, "private_environment_absent": True, "prefix": sys.prefix}
        result = {"content": [{"type": "text", "text": json.dumps(observation)}], "structuredContent": observation}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
