#!/usr/bin/env python3
"""M5 gates. Raw output stays outside the repository; no Goose suite."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BASE = json.loads((HERE / "baseline.json").read_text())
LOG = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/anchor-m5")
LOG.mkdir(parents=True, exist_ok=True)
ENV = dict(os.environ, CARGO_TARGET_DIR="/tmp/anchor-goose-regression-target", CARGO_BUILD_JOBS="2")


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def source_digest():
    paths = git("ls-tree", "-r", "--name-only", BASE["after"], "rust").decode().splitlines()
    return {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths if p.endswith((".rs", "Cargo.toml", "Cargo.lock"))}


result = {"baseline": BASE, "environment": {k: ENV[k] for k in ["CARGO_TARGET_DIR", "CARGO_BUILD_JOBS"]}, "checks": {}}
source_diff = git("diff", BASE["after"], "--", "rust")
if source_diff:
    raise SystemExit("Rust working tree differs from fixed baseline; validate an isolated snapshot instead")
before = source_digest()
delta = git("diff", BASE["m4"], BASE["after"], "--stat", "--", "rust/**/*.rs").decode()
result["rust_delta_since_m4"] = delta.splitlines()
(LOG / "rust-delta.patch").write_bytes(git("diff", BASE["m4"], BASE["after"], "--", "rust/**/*.rs"))
commands = [
    ("fmt", ["cargo", "fmt", "--all", "--", "--check"], ROOT / "rust"),
    ("fence", ["cargo", "test", "-p", "anchor-runner-host", "--test", "run_access_fence"], ROOT / "rust"),
]
if delta:
    commands.append(("workspace", ["cargo", "test", "--workspace", "--no-fail-fast"], ROOT / "rust"))
commands.append(("wording", ["bash", "scripts/check-sensitive-wording.sh"], ROOT))
for name, command, cwd in commands:
    path = LOG / (name + ".log")
    print("starting", name, flush=True)
    with path.open("wb") as output:
        done = subprocess.run(command, cwd=cwd, env=ENV, stdout=output, stderr=subprocess.STDOUT)
    raw = path.read_bytes()
    text = raw.decode(errors="replace")
    summaries = [line for line in text.splitlines() if line.startswith("test result:")]
    totals = [0, 0, 0]
    for line in summaries:
        match = re.search(r"(\d+) passed; (\d+) failed; (\d+) ignored", line)
        if match:
            totals = [a + int(b) for a, b in zip(totals, match.groups())]
    result["checks"][name] = dict(command=command, cwd=str(cwd), exit_code=done.returncode, log=str(path), sha256=hashlib.sha256(raw).hexdigest(), result_lines=summaries, totals=totals, result_count=len(summaries))
    print(name, done.returncode, totals, "results", len(summaries), flush=True)
kernel_diff = git("diff", BASE["kernel_baseline"], BASE["after"], "--", "rust/anchor-kernel")
result["c1"] = {"command": ["git", "diff", BASE["kernel_baseline"], BASE["after"], "--", "rust/anchor-kernel"], "diff_bytes": len(kernel_diff)}
after = source_digest()
result["source_unchanged_during_validation"] = after == before
result["source_files_checked"] = len(before)
result["source_manifest_sha256"] = hashlib.sha256(json.dumps(before, sort_keys=True).encode()).hexdigest()
result["source_diff_from_baseline_bytes"] = len(git("diff", BASE["after"], "--", "rust"))
result["web_diff_since_m4_bytes"] = len(git("diff", BASE["m4"], BASE["after"], "--", "apps/web"))
result["goose"] = {"rerun": False, "evidence": "../m4/verification.md", "targets": 41, "passed": 123, "failed": 0, "ignored": 0, "reason": "M5 §10.20 docs/experiments-only gate; RSI delta requires workspace, not full Goose"}
result_path = HERE / "checks.json"
prior = json.loads(result_path.read_text()) if result_path.exists() else {}
prior.update(result)
result_path.write_text(json.dumps(prior, ensure_ascii=False, indent=2) + "\n")
if kernel_diff or not result["source_unchanged_during_validation"] or any(c["exit_code"] for c in result["checks"].values()):
    raise SystemExit(1)
