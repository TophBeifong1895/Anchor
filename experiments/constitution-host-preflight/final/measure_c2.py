#!/usr/bin/env python3
"""Fixed-tree C2 ratchet evidence, with reviewed exclusions rather than save grep."""
import json
from pathlib import Path
import re
import subprocess
from measure_lines import BASE, HERE, ROOT, mask, test_partition

revision = BASE["after"]
sources, whole, spans = test_partition(revision)
save_hits, mutations, mutable_records, retired = [], [], [], []
for path, source in sources.items():
    if path in whole or path.split("/")[1] not in ("anchor-runner-host", "anchor-graph-host"):
        continue
    original = source.splitlines()
    for line, text in enumerate(mask(source).splitlines(), 1):
        if line in spans[path]:
            continue
        position = f"{path}:{line}"
        if re.search(r"\.save\s*\(", text):
            save_hits.append(dict(location=position, source=original[line - 1].strip()))
        if re.search(r"\b(?:run|record|expected|child|current|latest)\.(?:status|cursor|run_id|snapshot|input|plugin_bindings|plugin_bindings_initialized|results|edges|graph_calls|parallel|passes|invocations|decided|ceased|module_passes|module_activations|error|recovery|recovery_submissions|sequence|updated)\s*(?:[+\-*/]?=(?!=|>)|\.(?:push|insert|retain|clear|extend|remove)\s*\()", text):
            mutations.append(dict(location=position, source=original[line - 1].strip()))
        if re.search(r"&mut\s+GraphRunRecord", text):
            mutable_records.append(position)
        if re.search(r"\b(?:GraphRunner|RunStatus|FileRunStore|RunStore|RunCursor|stop_requested|pause_requested)\b", text):
            retired.append(position)
exclusions = {
    "rust/anchor-runner-host/src/api/settings.rs:43": "RuntimeConfiguration settings owner, not Run",
    "rust/anchor-runner-host/src/goose_acp/conversation.rs:246": "Native conversation branch owner",
    "rust/anchor-runner-host/src/goose_acp/conversation.rs:268": "Native conversation branch owner",
}
assert {x["location"] for x in save_hits} == set(exclusions), save_hits
assert not mutations and not mutable_records and not retired, (mutations, mutable_records, retired)
mapping = subprocess.check_output(["git", "show", f"{revision}:experiments/constitution-host-preflight/m3/c2-destinations.md"], cwd=ROOT, text=True)
save_rows = re.findall(r"^\| S\d{2} \|", mapping, re.M)
mutation_rows = re.findall(r"^\| M\d{2} \|", mapping, re.M)
assert len(save_rows) == 16 and len(mutation_rows) == 36
result = dict(revision=revision, h1=dict(save=16, mutation=36),
              after=dict(save=0, mutation=0), mapped_rows=dict(save=len(save_rows), mutation=len(mutation_rows)),
              mapping="../m3/c2-destinations.md", save_candidates=[dict(x, classification=exclusions[x["location"]]) for x in save_hits],
              field_mutation_candidates=mutations, mutable_record_arguments=mutable_records, retired_types=retired,
              review="M3's 52 destinations checked against fixed tree; M4 removed recovery/error/updated fields and moved history-derived recovery to RunView only; no persistent Run save API remains",
              limitations=["Lexical scan is supporting evidence, not Rust type resolution", "Mutation candidate variables and Graph fields are reviewable in script; constructors and immutable metadata are outside C2", "M3 line numbers are historical; use named functions, not the old numeric offsets"])
(HERE / "c2-counts.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(result, ensure_ascii=False, indent=2))
