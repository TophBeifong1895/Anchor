#!/usr/bin/env python3
"""Reproduce HostProbe::point_of against retained Kernel history, without writes.

Usage: probe_review.py DATA_ROOT ONLINE_SAMPLE [RAW_OUTPUT_DIRECTORY]
The first input is an existing, idle deterministic fixture data directory. Only
the derived sample and summary are committed; resource/history copies stay local.
Native rebuild (partial/missing projection and I/O failure) is tested separately.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BASE = json.loads((HERE / "baseline.json").read_text())


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(point):
    return json.dumps(point, sort_keys=True, separators=(",", ":"))


def load_lines(raw):
    return [json.loads(line) for line in raw.decode().splitlines() if line.strip()]


def point_of(record, validator):
    return dict(component=record["submission"]["target"].split("/")[0], validator=validator,
                principal=record["stamp"]["principal"]["kind"], action=record["submission"]["action"],
                effect=record["verdict"]["effect"], verification=record["verdict"]["verification"],
                reason=record["verdict"]["reason"], cost=record["stamp"]["cost"], knobs=record["stamp"]["knobs"])


def main():
    source, sample = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
    raw_dir = Path(sys.argv[3] if len(sys.argv) > 3 else "/tmp/anchor-m5/probe-snapshot")
    raw_dir.mkdir(parents=True, exist_ok=True)
    originals = {source / "kernel-probe/points.jsonl": (source / "kernel-probe/points.jsonl").read_bytes()}
    rebuilt = []
    for file in sorted((source / "kernel-runs").glob("*/resource.json")):
        resource_bytes = file.read_bytes()
        resource = json.loads(resource_bytes)
        if not resource["id"].startswith("run/") or resource["validator"] != "validator/graph":
            continue
        history_file = file.with_name("history.jsonl")
        history_bytes = history_file.read_bytes()
        originals[file], originals[history_file] = resource_bytes, history_bytes
        rebuilt.extend(point_of(record, resource["validator"]) for record in load_lines(history_bytes))
    if any(path.read_bytes() != data for path, data in originals.items()):
        raise SystemExit("data changed during read: use a quiescent fixture directory")
    for path, data in originals.items():
        target = raw_dir / path.relative_to(source)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    online = load_lines(originals[source / "kernel-probe/points.jsonl"])
    a, b = Counter(map(canonical, online)), Counter(map(canonical, rebuilt))
    rebuilt_bytes = "".join(json.dumps(point, separators=(",", ":")) + "\n" for point in rebuilt).encode()
    (raw_dir / "rebuilt-points.jsonl").write_bytes(rebuilt_bytes)
    fresh = load_lines(sample.read_bytes())
    # These are only measurement coordinates, never history or model output.
    shutil.copyfile(sample, HERE / "probe-sample.jsonl")
    keys = ["validator", "principal", "action", "effect", "verification", "reason"]
    joint = Counter(tuple(point[k] for k in keys) for point in fresh)
    probe_source = subprocess.check_output(["git", "show", f'{BASE["after"]}:rust/anchor-runner-host/src/application/run_access/kernel_probe.rs'], cwd=ROOT)
    report = dict(baseline=BASE["after"], native_projection_source_sha256=sha(probe_source),
                  retained_fixture=str(source), raw_snapshot=str(raw_dir),
                  files={str(path.relative_to(source)): sha(data) for path, data in originals.items()},
                  reconstruction=dict(online_points=len(online), rebuilt_points=len(rebuilt),
                                      ordered_equal=online == rebuilt, multiset_equal=a == b,
                                      missing_in_rebuild=[json.loads(x) for x in (a - b).elements()],
                                      extra_in_rebuild=[json.loads(x) for x in (b - a).elements()],
                                      differences=[] if a == b else ["point multiset differs"],
                                      comparison="All nine Point fields; record order per resource, multiset across resources"),
                  sample=dict(source="host_probe_records_create_submit_and_scheduler_verdicts; actual KernelRuns parallel local fixture; no Goose or model Provider", path="probe-sample.jsonl", sha256=sha(sample.read_bytes()), total_points=len(fresh), total_cost=sum(p["cost"] for p in fresh),
                              **{"by_" + key: dict(sorted(Counter(p[key] for p in fresh).items())) for key in keys},
                              joint=[dict(zip(keys, values), count=count) for values, count in sorted(joint.items())]),
                  limitations=["Point omits Run id/seq/time, so no cross-Run chronological order can be reconstructed", "Deleted owners have no retained history; rebuild only covers retained Run owners", "Best-effort append can lose projection writes; native reopen rebuilds from retained history", "No concurrent-writer or crash stress test claimed; retained fixture verified byte-stable during read"])
    (HERE / "probe-summary.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"reconstruction": report["reconstruction"], "sample": report["sample"]}, ensure_ascii=False, indent=2))
    if a != b:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
