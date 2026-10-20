#!/usr/bin/env python3
"""Verify the hand-reviewed M5 rule inventory against immutable Git blobs.

Run from any directory: python3 experiments/constitution-host-preflight/final/rules_audit.py
No working-tree Rust files, logs, network, build or test execution are used.
Semantic counts are reviewed input, not a regex-derived proof of completeness.
"""
import hashlib
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[3]
BEFORE = "1d1ba54035b2f25a45fd52c2536f5dc19cbd85e2"
AFTER = "e60374fd06eda07de8df9c41e64a5e03f6e3246d"
PREFIX = {
    "R": "rust/anchor-runner-host/src/",
    "G": "rust/anchor-graph-host/src/",
    "T": "rust/anchor-runtime/src/",
    "K": "rust/anchor-kernel/src/",
    "W": "apps/web/src/",
}

def blob(revision, path):
    return subprocess.check_output(["git", "show", f"{revision}:{path}"], cwd=ROOT)

# Each expected fragment is checked at the exact reported line. Multiple
# consumers are intentionally absent. M1.5 before sites are all verified below.
CURRENT = [
    ("R", "application/run_access/run_view.rs", 22, "pause: self.phase"),
    ("R", "application/run_access/run_view.rs", 23, "stop: self.phase"),
    ("R", "application/run_access/run_view.rs", 24, "resume: (self.phase"),
    ("R", "application/run_access/run_view.rs", 32, "abandon: !self.is_settled()"),
    ("R", "application/run_access/run_view.rs", 35, "fn is_settled"),
    ("R", "application/run_access/projection.rs", 7, "fn projected_run_is_unfinished"),
    ("R", "application/run_access/projection.rs", 67, "fn project_run"),
    ("R", "application/run_access/kernel_runs.rs", 184, "fn ensure_idle"),
    ("R", "application/run_access/kernel_runs.rs", 202, "fn register"),
    ("R", "application/run_access/kernel_runs.rs", 497, "fn kernel_control"),
    ("R", "application/run_access/kernel_runs.rs", 544, "resource.phase() != Some(&Phase::Executing)"),
    ("R", "application/run_access/kernel_runs.rs", 615, "resource.phase() == Some(&Phase::Executing)"),
    ("R", "application/run_access/kernel_runs/pause.rs", 43, "resource.phase() == Some(&Phase::Executing)"),
    ("R", "application/run_access/kernel_runs/pause.rs", 90, "fn request_pause"),
    ("R", "application/run_access.rs", 225, "fn is_quiescent"),
    ("R", "application/run_access.rs", 240, "fn needs_attention"),
    ("R", "application/run_access.rs", 262, "fn has_stopped_execution"),
    ("R", "application/run_access.rs", 268, "fn can_dispatch"),
    ("R", "application/run_access.rs", 281, "fn waits_for_invocation"),
    ("R", "application/run_access.rs", 315, "fn occupies_conversation_execution"),
    ("R", "application/run_access.rs", 338, "fn validate_control_identity"),
    ("R", "application/abandon.rs", 130, "RunPhase::Settled"),
    ("R", "application/run_access/control.rs", 57, "if let Some((node, invocation))"),
    ("R", "application/run_access/kernel_calls.rs", 241, "if matches!("),
    ("R", "application/run_access/kernel_calls.rs", 310, "resource.phase() != Some(&Phase::Executing)"),
    ("R", "application/run_access/kernel_calls.rs", 333, "fn kernel_child_finished"),
    ("R", "application/run_access/kernel_runs/recovery.rs", 13, "fn observe_recovery_attempts"),
    ("R", "application/run_access/kernel_runs/recovery.rs", 90, "fn project_recovery"),
    ("R", "application/run_access/kernel_runs/recovery.rs", 153, "fn recover_attempt"),
    ("R", "application/run_access/kernel_runs/recovery.rs", 223, "let operation = if decision"),
    ("R", "application/run_access/recovery.rs", 70, "fn inspect_interrupted_attempts"),
    ("R", "node_host/facts.rs", 45, "if read_optional"),
    ("R", "node_host/execution.rs", 191, "if crate::run_data::legacy_invocation_exists"),
    ("R", "node_host/execution.rs", 207, "host and Goose node facts conflict"),
    ("R", "node_host/execution.rs", 239, "nodes.pending_recovery"),
    ("R", "goose_acp.rs", 270, "InvocationFailure::Uncertain"),
    ("R", "goose_acp.rs", 399, "None => CompletionFact::Failed"),
    ("R", "goose_acp.rs", 411, "match self.completion_fact"),
    ("R", "goose_acp/recovery.rs", 6, "fn attempt"),
    ("R", "goose_acp/recovery.rs", 34, "fn record_decision"),
    ("G", "graph_call.rs", 319, "let resumable"),
    ("G", "graph_call.rs", 535, "fn project_child_outcome"),
    ("T", "graph/scheduler.rs", 49, "fn submit_with_retry"),
    ("T", "graph/scheduler.rs", 198, "Reason::TaskWaiting | Reason::TaskSettled"),
    ("T", "graph/scheduler/execution.rs", 162, "NodeExecutionOutcome::BudgetExhausted"),
    ("T", "graph/scheduler/calls.rs", 145, "fn observe_child"),
    ("K", "model.rs", 305, "fn is_settled"),
    ("K", "kernel.rs", 213, "if waiting_work"),
    ("K", "kernel.rs", 227, "Phase::Settled"),
    ("K", "kernel.rs", 234, "submission.seen != resource.version"),
    ("K", "kernel.rs", 504, '"stop" =>'),
    ("K", "kernel.rs", 521, '"resume" =>'),
    ("K", "kernel.rs", 538, '"abandon" =>'),
    ("W", "runPresentation.ts", 10, "runNeedsAttention"),
    ("W", "runPresentation.ts", 12, "runQuiescent"),
]
EXTRA_BEFORE = [
    ("R", "api/schedules.rs", 316, "let unfinished = matches!("),
    ("R", "application/dispatch.rs", 78, "record.status == RunStatus::Ready"),
    ("R", "node_host/execution.rs", 191, "if crate::run_data::legacy_invocation_exists"),
]
COUNTS = {
    "terminal_lifecycle": [1, 1],
    "unfinished_json_boundary": [1, 1],
    "stop": [4, 1],
    "resume_action_advice": [2, 1],
    "resume_worker_gate": [3, 1],
    "resume_terminal_write_gate": [1, 0],
    "resume_pending_write_gate": [1, 0],
    "resume_budget_write_gate": [1, 0],
    "resume_explicit_inspection": [1, 1],
    "resume_child_callback": [1, 1],
    "retained_dormant_child_binding_source": [0, 1],
    "resume_child_drive_selection": [1, 1],
    "resume_kernel_control_or_drive": [1, 1],
    "abandon_advice": [0, 1],
    "abandon_command_gate": [2, 1],
    "abandon_final_write_gate": [1, 0],
    "pause": [5, 1],
    "quiescent": [2, 1],
    "attention": [2, 1],
    "late_work_host_permission": [0, 0],
    "late_child_callback": [1, 1],
    "unknown_started_without_completion": [1, 1],
    "unknown_host_goose_conflict": [1, 1],
    "unknown_fixture_missing_completion": [1, 1],
    "unknown_goose_no_replay": [1, 1],
    "unknown_transport_failure_classification": [1, 1],
    "unknown_graph_child_drive_selection": [1, 1],
    "unknown_child_executor_human_wait_park": [0, 1],
    "unknown_graph_child_outcome_projection": [1, 1],
    "unknown_detached_dispatch_and_startup_scan": [2, 2],
    "unknown_old_runner_exception_write": [1, 0],
    "unknown_explicit_inspection": [1, 1],
    "unknown_recovery_target": [2, 1],
    "unknown_recovery_decision_control_selection": [1, 1],
    "unknown_legacy_agent_fact": [1, 1],
    "unknown_native_attempt_identification": [0, 1],
    "unknown_native_decision_validation": [0, 1],
    "unknown_native_pre_dispatch_gate": [0, 1],
    "unknown_native_observation_registration": [0, 1],
    "unknown_accepted_history_projection": [0, 1],
    "budget_resume_extra_gate": [1, 0],
    "budget_child_drive_filter": [1, 0],
    "budget_child_wait_projection": [1, 1],
    "release_child_projection": [1, 1],
    "release_parent_callback": [2, 1],
}

def main():
    prior = json.loads(blob(AFTER, "experiments/constitution-host-preflight/m15/counts.json"))
    checked, hashes = [], {}
    def verify(revision, path, line, expected):
        raw = blob(revision, path)
        actual = raw.decode().splitlines()[line - 1].strip()
        if expected.strip() not in actual:
            raise SystemExit(f"source mismatch: {revision}:{path}:{line}: {actual!r}")
        hashes[f"{revision}:{path}"] = hashlib.sha256(raw).hexdigest()
        checked.append(f"{revision}:{path}:{line}")
    for rule in prior["rules"]:
        for site in rule["before"]["sites"]:
            verify(BEFORE, site["path"], site["line"], site["source_line"])
    for revision, sites in [(BEFORE, EXTRA_BEFORE), (AFTER, CURRENT)]:
        for prefix, path, line, expected in sites:
            verify(revision, PREFIX[prefix] + path, line, expected)
    print(json.dumps({
        "before": BEFORE, "after": AFTER,
        "unit": "reviewed independent rule body; consumers excluded; do not sum unlike subrules",
        "baseline_original_counts": {r["id"]: r["before"]["host_run_count"] for r in prior["rules"]},
        "comparable_subrules": COUNTS,
        "checked_anchor_count": len(checked),
        "source_sha256": hashes,
        "checked_anchors": checked,
    }, ensure_ascii=False, indent=2))

if __name__ == "__main__":
    main()
