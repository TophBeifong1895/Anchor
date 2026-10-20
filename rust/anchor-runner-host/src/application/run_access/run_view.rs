//! Read-only lifecycle facts. No Session policy or writable Run state lives here.
use anchor_kernel::{Phase, Waiter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct AvailableActions {
    pub(crate) pause: bool,
    pub(crate) stop: bool,
    pub(crate) resume: bool,
    pub(crate) abandon: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct RunView {
    pub(crate) phase: Phase,
    pub(crate) worker: bool,
    pub(crate) updated: Option<String>,
    /// Optional Host explanation for child/effect waits.
    pub(crate) reason: Option<String>,
}
impl RunView {
    pub(crate) fn actions(&self) -> AvailableActions {
        AvailableActions {
            pause: self.phase == Phase::Executing && self.worker,
            stop: self.phase == Phase::Executing,
            resume: (self.phase == Phase::Executing
                || matches!(
                    self.phase,
                    Phase::Waiting {
                        waiter: Waiter::Human { .. }
                    }
                ))
                && !self.worker,
            abandon: !self.is_settled(),
        }
    }
    pub(crate) fn is_settled(&self) -> bool {
        matches!(self.phase, Phase::Settled { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_kernel::{Outcome, Reason};
    #[test]
    fn actions_cover_every_phase_waiter_and_outcome_with_and_without_worker() {
        let phases = [
            Phase::Executing,
            Phase::Waiting {
                waiter: Waiter::Human {
                    reason: "verify".into(),
                },
            },
            Phase::Waiting {
                waiter: Waiter::Child {
                    task: "child".into(),
                },
            },
            Phase::Waiting {
                waiter: Waiter::Effect {
                    effect: "effect".into(),
                },
            },
            Phase::Settled {
                result: Outcome::Done,
            },
            Phase::Settled {
                result: Outcome::Failed {
                    reason: Reason::ContentInvalid,
                    detail: "failed".into(),
                },
            },
            Phase::Settled {
                result: Outcome::Escalated {
                    detail: "budget".into(),
                },
            },
            Phase::Settled {
                result: Outcome::Unknown {
                    detail: "unknown".into(),
                },
            },
            Phase::Settled {
                result: Outcome::Aborted {
                    detail: "abandon".into(),
                },
            },
        ];
        for (index, phase) in phases.into_iter().enumerate() {
            for worker in [false, true] {
                let view = RunView {
                    phase: phase.clone(),
                    worker,
                    updated: None,
                    reason: None,
                };
                assert_eq!(
                    view.actions(),
                    AvailableActions {
                        pause: index == 0 && worker,
                        stop: index == 0,
                        resume: index < 2 && !worker,
                        abandon: index < 4
                    },
                    "{phase:?} worker={worker}"
                );
            }
        }
    }
}
