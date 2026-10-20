#!/usr/bin/env python3
"""Replay the existing routing.rs standalone fixture with a retained data root.

No new probe or production hook. A single true Op executes through the real Host
and sandbox; starting the settled Run again opens the same Kernel owner and
repairs a damaged/missing disposable projection without replaying the Op.
"""
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/anchor-goose-regression-target/debug/anchor-runner-host")
RAW = Path(tempfile.mkdtemp(prefix="probe-retained-", dir="/tmp/anchor-m5"))
bundle = RAW / "bundle"
bundle.mkdir()
graph = {"objective": "M5 retained deterministic probe review", "entry": "work", "ops": {"work": {"run": "true"}}, "nodes": [{"id": "work", "op": "work"}], "edges": []}
(bundle / "graph.json").write_text(json.dumps(graph))
(bundle / "manifest.json").write_text(json.dumps({"format": 1, "graph": "graph.json", "plugins": []}))
env = dict(os.environ, ANCHOR_RUNNER_STATE_ROOT=str(RAW / "state"), ANCHOR_RUNNER_WORKSPACE_ROOT=str(RAW / "work"), ANCHOR_RUNNER_BUNDLE_ROOT=str(bundle), ANCHOR_RUNNER_ALLOWED_COMMANDS="sh,true")
for key in ["ANCHOR_RUNNER_LIBRARY_ROOT", "ANCHOR_MODEL_API_KEY", "ANCHOR_MODEL_URL", "ANCHOR_MODEL_NAME", "ANCHOR_RUST_MCP_SERVERS"]:
    env.pop(key, None)
request = json.dumps({"op": "start_bundle", "version": 1, "request_id": "m5-probe-retained", "run_id": "m5-probe-retained", "input": {}}).encode()


def invoke(label):
    output = subprocess.run([str(BINARY)], input=struct.pack(">I", len(request)) + request, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=ROOT, timeout=60)
    (RAW / (label + "-stdout.bin")).write_bytes(output.stdout)
    (RAW / (label + "-stderr.txt")).write_bytes(output.stderr)
    assert output.returncode == 0, output.stderr.decode(errors="replace")
    assert struct.unpack(">I", output.stdout[:4])[0] == len(output.stdout[4:])
    response = json.loads(output.stdout[4:])
    assert response["kind"] == "run", response
    assert response["projection"]["outcome"] == "done", response
    return response


first = invoke("first")
state = RAW / "state"
point = state / "kernel-probe/points.jsonl"
resource = state / "kernel-runs/run%2Fm5-probe-retained/resource.json"
history = resource.with_name("history.jsonl")
online, before_resource, before_history = point.read_bytes(), resource.read_bytes(), history.read_bytes()
(RAW / "online-points.jsonl").write_bytes(online)
assert json.loads(before_resource)["state"]["format"] == 15
point.write_bytes(b"{partial\n")
invoke("partial-reopen")
partial_equal = point.read_bytes() == online
point.unlink()
invoke("missing-reopen")
missing_equal = point.read_bytes() == online
facts_equal = resource.read_bytes() == before_resource and history.read_bytes() == before_history
assert partial_equal and missing_equal and facts_equal
summary = dict(data_root=str(state), raw_output=str(RAW), binary=str(BINARY), binary_sha256=hashlib.sha256(BINARY.read_bytes()).hexdigest(), graph_schema=15,
               point_count=len(online.splitlines()), online_sha256=hashlib.sha256(online).hexdigest(), history_sha256=hashlib.sha256(before_history).hexdigest(),
               partial_rebuilt_identically=partial_equal, missing_rebuilt_identically=missing_equal, resource_and_history_unchanged=facts_equal,
               scope="Existing deterministic routing.rs fixture pattern; actual framed Host/KernelScheduler/Op/Sandbox; no Goose or model Provider")
(HERE / "probe-fixture.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(summary, indent=2))
