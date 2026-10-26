#!/usr/bin/env python3
"""Measure H5 baseline or an immutable implementation tree with the same partition."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "experiments/constitution-host-preflight/final/measure_lines.py"
spec = importlib.util.spec_from_file_location("measure_lines", SOURCE)
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


revision = git("rev-parse", sys.argv[1] if len(sys.argv) > 1 else "fcda48c")
sources, whole, spans = measure.test_partition(revision)
files = {
    path: len(source.splitlines()) - len(spans[path])
    for path, source in sources.items()
    if path.startswith("rust/anchor-runner-host/") and path not in whole
}
result = {
    "phase": "inventory_only" if revision == git("rev-parse", "fcda48c") else "implementation",
    "baseline": git("rev-parse", "fcda48c"),
    "measured_revision": revision,
    "source_tree": git("rev-parse", revision + ":rust/anchor-runner-host/src"),
    "measurement": "measure_lines.py::test_partition",
    "production_physical_lines": sum(files.values()),
    "by_file": files,
}
output = Path(__file__).with_name(sys.argv[2] if len(sys.argv) > 2 else "baseline-lines.json")
output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
print(result["production_physical_lines"])
