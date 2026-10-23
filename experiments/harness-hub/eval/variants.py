"""Instruction variants an experiment arm can run with: each is one sentence
a proposal adds to one task's instructions, placed before the completion
text. The task, its inputs, expected values and grader stay as they are, so
the task version does not change; the arm's AgentNode configuration records
the changed instructions digest."""

from spec import COMPLETION

# From the RSI report of p7-blind-03, proposals P2, P3 and P4.
VARIANTS = {
    'P2': ('audit',
           'An open order with no payment or refund record at all has net 0: it belongs in '
           '`unpaid` and not in `short`, and it owes its full amount_cny toward '
           '`top_outstanding`.'),
    'P3': ('incident',
           '`first_error_utc` counts every end event on its own, including an error end of a '
           'request that a retry later ended below 500; the final status (the status of the '
           'last end) is used only for `server_errors_by_service`.'),
    'P4': ('refactor',
           'Write all three modules first, then run the tests once over all of them and read the '
           'outputs back once; do not rerun the tests after each file or each small edit.'),
}


def instructions(task, names):
    """`task`'s instructions with the sentences of the variants `names` that
    apply to it."""
    added = [VARIANTS[n][1] for n in names if VARIANTS[n][0] == task.id]
    if not added:
        return task.instructions
    assert task.instructions.endswith(COMPLETION), task.id
    head = task.instructions[:-len(COMPLETION)]
    return head + ' '.join(added) + ' ' + COMPLETION
