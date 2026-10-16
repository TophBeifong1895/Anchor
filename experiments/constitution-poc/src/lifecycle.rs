//! E2: does the Run lifecycle collapse into the constitution?
//!
//! `Documented` transcribes the current rules for stop / resume / retire /
//! abandon from docs/wecom-assistant.md (停止、恢复与退役; 插件/图更新的操作顺序)
//! and `reject_terminal_control` in application/control.rs. Where the docs are
//! silent the choice is marked `ASSUMPTION`.
//!
//! `Derived` has no lifecycle rules of its own. It is a Task (executing,
//! paused, or settled once with an outcome), a "current instance" pointer
//! owned by the Session, and an effect lifecycle, with these laws only:
//!   L2a settled is irreversible;      L2b an unknown effect blocks until observed;
//!   L3  a task settles exactly once;  Q   quiescent = not executing.
//!
//! The explorer walks the product of both machines over every reachable state
//! and every event and reports each place where they disagree.

use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Event {
    Stop,
    Resume,
    Retire,
    Abandon,
    Finish,
    Crash,
    DeliverStart,
    DeliverConfirm,
    DeliverLost,
    Inspect,
}

pub const EVENTS: [Event; 10] = [
    Event::Stop,
    Event::Resume,
    Event::Retire,
    Event::Abandon,
    Event::Finish,
    Event::Crash,
    Event::DeliverStart,
    Event::DeliverConfirm,
    Event::DeliverLost,
    Event::Inspect,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Delivery {
    None,
    Pending,
    Unknown,
}

/// What an operator or user can observe, compared between the two models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Observable {
    pub executing: bool,
    pub resumable: bool,
    /// Completed / Failed / Aborted, as the API reports a terminal Run.
    pub terminal: Option<&'static str>,
    pub current: bool,
    pub retired: bool,
    pub delivery: Delivery,
}

pub trait Machine: Clone + Ord {
    /// `None` = refused; `Some(next)` = accepted.
    fn step(&self, event: Event) -> Option<Self>;
    fn observe(&self) -> Observable;
}

// ---------------------------------------------------------------------------
// The documented rules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Running,
    Stopped,
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Documented {
    pub status: Status,
    pub retired: bool,
    pub current: bool,
    pub delivery: Delivery,
}

impl Documented {
    pub fn initial() -> Self {
        Self {
            status: Status::Running,
            retired: false,
            current: true,
            delivery: Delivery::None,
        }
    }
}

impl Machine for Documented {
    fn step(&self, event: Event) -> Option<Self> {
        let mut next = *self;
        match event {
            Event::Stop => match self.status {
                Status::Running => next.status = Status::Stopped,
                // ASSUMPTION A1: stopping a stopped Run is an idempotent no-op.
                Status::Stopped => {}
                // control.rs: "terminal Run cannot be controlled".
                _ => return None,
            },
            // "retired Run 禁止 resume"; Aborted resume returns 409.
            Event::Resume => {
                if self.status != Status::Stopped || self.retired {
                    return None;
                }
                next.status = Status::Running;
            }
            Event::Retire => {
                // "幂等重试不创建实例"
                if self.retired {
                    return Some(next);
                }
                // ASSUMPTION A3: only the current instance can be retired.
                // Requires Stopped or terminal, not active, no Running Turn and
                // no pending / sending / unknown delivery.
                if !self.current
                    || self.status == Status::Running
                    || self.delivery != Delivery::None
                {
                    return None;
                }
                next.retired = true;
                next.current = false;
            }
            Event::Abandon => match self.status {
                // "Completed/Failed 返回 409"
                Status::Completed | Status::Failed => return None,
                // "同一 Run 重复调用返回同一 body"
                Status::Aborted => {}
                // ASSUMPTION A4: abandon is not blocked by deliveries or retirement.
                _ => next.status = Status::Aborted,
            },
            Event::Finish => {
                if self.status != Status::Running {
                    return None;
                }
                next.status = Status::Completed;
            }
            Event::Crash => {
                if self.status != Status::Running {
                    return None;
                }
                next.status = Status::Failed;
            }
            Event::DeliverStart => {
                if self.status != Status::Running || self.delivery != Delivery::None {
                    return None;
                }
                next.delivery = Delivery::Pending;
            }
            Event::DeliverConfirm | Event::DeliverLost => {
                if self.delivery != Delivery::Pending {
                    return None;
                }
                next.delivery = if event == Event::DeliverConfirm {
                    Delivery::None
                } else {
                    Delivery::Unknown
                };
            }
            Event::Inspect => {
                if self.delivery != Delivery::Unknown {
                    return None;
                }
                next.delivery = Delivery::None;
            }
        }
        Some(next)
    }

    fn observe(&self) -> Observable {
        Observable {
            executing: self.status == Status::Running,
            resumable: self.status == Status::Stopped && !self.retired,
            terminal: match self.status {
                Status::Completed => Some("completed"),
                Status::Failed => Some("failed"),
                Status::Aborted => Some("aborted"),
                _ => None,
            },
            current: self.current,
            retired: self.retired,
            delivery: self.delivery,
        }
    }
}

// ---------------------------------------------------------------------------
// The same behaviour derived from the constitution
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Outcome {
    Done,
    Failed,
    Aborted,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Executing,
    Paused,
    Settled(Outcome),
}

/// The Session owns the pointer; the Run owns the task. One writer each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pointer {
    Current,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Derived {
    pub phase: Phase,
    pub pointer: Pointer,
    pub effect: Delivery,
}

impl Derived {
    pub fn initial() -> Self {
        Self {
            phase: Phase::Executing,
            pointer: Pointer::Current,
            effect: Delivery::None,
        }
    }

    /// L2a + L3: a settled task never changes again; settling the same way is
    /// a replay of the same fact.
    fn settle(&self, outcome: Outcome) -> Option<Self> {
        match self.phase {
            Phase::Settled(existing) if existing == outcome => Some(*self),
            Phase::Settled(_) => None,
            _ => Some(Self {
                phase: Phase::Settled(outcome),
                ..*self
            }),
        }
    }

    /// The only change a task allows besides settling: pause or unpause.
    fn write_phase(&self, to: Phase) -> Option<Self> {
        match (self.phase, to) {
            (Phase::Settled(_), _) => None,
            (from, to) if from == to => Some(*self),
            _ => Some(Self { phase: to, ..*self }),
        }
    }
}

impl Machine for Derived {
    fn step(&self, event: Event) -> Option<Self> {
        match event {
            Event::Stop => self.write_phase(Phase::Paused),
            Event::Resume => match self.phase {
                Phase::Paused => self.write_phase(Phase::Executing),
                _ => None,
            },
            Event::Abandon => self.settle(Outcome::Aborted),
            Event::Finish | Event::Crash => match self.phase {
                Phase::Executing => self.settle(if event == Event::Finish {
                    Outcome::Done
                } else {
                    Outcome::Failed
                }),
                _ => None,
            },
            // Retirement = the Session clears its pointer. Precondition is Q
            // plus L2b; an unsettled task is settled as retired so it can
            // never execute again (L2a).
            Event::Retire => {
                if self.pointer == Pointer::Retired {
                    return Some(*self);
                }
                if self.phase == Phase::Executing || self.effect != Delivery::None {
                    return None;
                }
                let settled = match self.phase {
                    Phase::Paused => self.settle(Outcome::Retired)?,
                    _ => *self,
                };
                Some(Self {
                    pointer: Pointer::Retired,
                    ..settled
                })
            }
            // Effect lifecycle: authorized → started → observed | unknown.
            Event::DeliverStart => {
                if self.phase != Phase::Executing || self.effect != Delivery::None {
                    return None;
                }
                Some(Self {
                    effect: Delivery::Pending,
                    ..*self
                })
            }
            Event::DeliverConfirm | Event::DeliverLost => {
                if self.effect != Delivery::Pending {
                    return None;
                }
                Some(Self {
                    effect: if event == Event::DeliverConfirm {
                        Delivery::None
                    } else {
                        Delivery::Unknown
                    },
                    ..*self
                })
            }
            Event::Inspect => {
                if self.effect != Delivery::Unknown {
                    return None;
                }
                Some(Self {
                    effect: Delivery::None,
                    ..*self
                })
            }
        }
    }

    fn observe(&self) -> Observable {
        Observable {
            executing: self.phase == Phase::Executing,
            resumable: self.phase == Phase::Paused,
            terminal: match self.phase {
                Phase::Settled(Outcome::Done) => Some("completed"),
                Phase::Settled(Outcome::Failed) => Some("failed"),
                Phase::Settled(Outcome::Aborted) => Some("aborted"),
                _ => None,
            },
            current: self.pointer == Pointer::Current,
            retired: self.pointer == Pointer::Retired,
            delivery: self.effect,
        }
    }
}

// ---------------------------------------------------------------------------
// Exhaustive product exploration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Divergence {
    /// Shortest event sequence reaching the divergence, ending with the event
    /// that disagrees.
    pub path: Vec<Event>,
    pub left_accepts: bool,
    pub right_accepts: bool,
    pub left_after: Option<Observable>,
    pub right_after: Option<Observable>,
}

#[derive(Clone, Debug)]
pub struct Exploration {
    pub states: usize,
    pub transitions: usize,
    pub divergences: Vec<Divergence>,
}

/// Breadth-first over the product. Only agreeing transitions are followed, so
/// every reported divergence is reached by a path on which both models agree.
pub fn explore<L: Machine, R: Machine>(left: L, right: R) -> Exploration {
    let mut seen: BTreeMap<(L, R), Vec<Event>> = BTreeMap::new();
    let mut queue = VecDeque::new();
    seen.insert((left.clone(), right.clone()), Vec::new());
    queue.push_back((left, right));
    let mut transitions = 0;
    let mut divergences = Vec::new();
    while let Some((l, r)) = queue.pop_front() {
        let path = seen[&(l.clone(), r.clone())].clone();
        for event in EVENTS {
            transitions += 1;
            let (ln, rn) = (l.step(event), r.step(event));
            let agree = match (&ln, &rn) {
                (None, None) => true,
                (Some(a), Some(b)) => a.observe() == b.observe(),
                _ => false,
            };
            let mut next_path = path.clone();
            next_path.push(event);
            if !agree {
                divergences.push(Divergence {
                    path: next_path,
                    left_accepts: ln.is_some(),
                    right_accepts: rn.is_some(),
                    left_after: ln.as_ref().map(Machine::observe),
                    right_after: rn.as_ref().map(Machine::observe),
                });
                continue;
            }
            if let (Some(a), Some(b)) = (ln, rn)
                && let std::collections::btree_map::Entry::Vacant(slot) =
                    seen.entry((a.clone(), b.clone()))
            {
                slot.insert(next_path);
                queue.push_back((a, b));
            }
        }
    }
    Exploration {
        states: seen.len(),
        transitions,
        divergences,
    }
}

/// Divergences grouped by (last event, which side accepted), keeping the
/// shortest path of each group.
pub fn group(divergences: &[Divergence]) -> Vec<&Divergence> {
    let mut best: BTreeMap<(Event, bool, bool), &Divergence> = BTreeMap::new();
    for d in divergences {
        let key = (*d.path.last().unwrap(), d.left_accepts, d.right_accepts);
        best.entry(key)
            .and_modify(|e| {
                if d.path.len() < e.path.len() {
                    *e = d;
                }
            })
            .or_insert(d);
    }
    best.into_values().collect()
}
