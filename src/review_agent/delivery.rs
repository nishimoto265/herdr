use std::collections::{HashMap, VecDeque};

use crate::detect::{Agent, AgentState};
use crate::layout::PaneId;

use super::conversation::{
    CompletionReadiness, ConversationProvider, TranscriptBinding, TranscriptCheckpoint,
    TranscriptResolution,
};

const PERSISTED_DELIVERY_VERSION: u32 = 1;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedReviewDelivery {
    version: u32,
    fronts: Vec<PersistedFrontDelivery>,
    backends: Vec<PersistedBackendDelivery>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedFrontDelivery {
    front_pane_id: u32,
    backside_pane_id: u32,
    generation: u64,
    #[serde(default)]
    armed: bool,
    #[serde(default)]
    phase: FrontPhase,
    #[serde(default)]
    pending_phases: Vec<(u64, FrontPhase)>,
    acknowledged_checkpoint: Option<TranscriptCheckpoint>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedBackendDelivery {
    backside_pane_id: u32,
    queue: Vec<PersistedDeliveryJob>,
    in_flight: Option<PersistedDeliveryJob>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedDeliveryJob {
    front_pane_id: u32,
    backside_pane_id: u32,
    generation: u64,
    binding: TranscriptBinding,
    completed: TranscriptCheckpoint,
}

impl From<&DeliveryJob> for PersistedDeliveryJob {
    fn from(job: &DeliveryJob) -> Self {
        Self {
            front_pane_id: job.assignment.front_pane_id.raw(),
            backside_pane_id: job.assignment.backside_pane_id.raw(),
            generation: job.assignment.generation,
            binding: job.binding.clone(),
            completed: job.completed.clone(),
        }
    }
}

impl PersistedDeliveryJob {
    fn into_job(self) -> DeliveryJob {
        DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id: PaneId::from_raw(self.front_pane_id),
                backside_pane_id: PaneId::from_raw(self.backside_pane_id),
                generation: self.generation,
            },
            binding: self.binding,
            completed: self.completed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentIdentity {
    pub(crate) front_pane_id: PaneId,
    pub(crate) backside_pane_id: PaneId,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryJob {
    pub(crate) assignment: AssignmentIdentity,
    pub(crate) binding: TranscriptBinding,
    pub(crate) completed: TranscriptCheckpoint,
}

impl DeliveryJob {
    pub(crate) fn source_event_id(&self) -> String {
        format!(
            "front-{}-generation-{}-offset-{}",
            self.assignment.front_pane_id.raw(),
            self.assignment.generation,
            self.completed.byte_offset
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeliveryAction {
    ResolveTranscript {
        assignment: AssignmentIdentity,
        provider: ConversationProvider,
    },
    ProbeCompletion {
        assignment: AssignmentIdentity,
        binding: TranscriptBinding,
    },
    StartBackend {
        backside_pane_id: PaneId,
    },
    RestartBackendAfterExit {
        backside_pane_id: PaneId,
    },
    SendRole {
        backside_pane_id: PaneId,
    },
    SendConversation(DeliveryJob),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendLifecycle {
    Unassigned,
    Starting,
    Ready,
    Busy,
    Failed,
    Restarting,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FrontPhase {
    #[default]
    Idle,
    Binding {
        provider: ConversationProvider,
        idle_after_bind: bool,
    },
    Working {
        binding: TranscriptBinding,
    },
    Probing {
        binding: TranscriptBinding,
    },
}

#[derive(Clone, Debug)]
struct FrontDelivery {
    backside_pane_id: PaneId,
    generation: u64,
    armed: bool,
    phase: FrontPhase,
    pending_phases: Vec<(u64, FrontPhase)>,
    acknowledged_checkpoint: Option<TranscriptCheckpoint>,
}

#[derive(Clone, Debug)]
enum BackendInFlight {
    Role,
    Conversation(DeliveryJob),
}

#[derive(Clone, Debug)]
struct BackendDelivery {
    lifecycle: BackendLifecycle,
    role_delivered: bool,
    queue: VecDeque<DeliveryJob>,
    in_flight: Option<BackendInFlight>,
}

impl Default for BackendDelivery {
    fn default() -> Self {
        Self {
            lifecycle: BackendLifecycle::Unassigned,
            role_delivered: false,
            queue: VecDeque::new(),
            in_flight: None,
        }
    }
}

/// Pure delivery state. PTY spawning, transcript IO, and byte writes are
/// represented as actions and performed by the App runtime.
#[derive(Default)]
pub(crate) struct ReviewDeliveryState {
    fronts: HashMap<PaneId, FrontDelivery>,
    backends: HashMap<PaneId, BackendDelivery>,
}

impl ReviewDeliveryState {
    pub(crate) fn has_in_flight_source_event(&self, source_event_id: &str) -> bool {
        self.backends.values().any(|backend| {
            matches!(
                &backend.in_flight,
                Some(BackendInFlight::Conversation(job))
                    if job.source_event_id() == source_event_id
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_in_flight_conversation() -> (Self, String) {
        let front_pane_id = PaneId::alloc();
        let backside_pane_id = PaneId::alloc();
        let job = DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id,
                backside_pane_id,
                generation: 7,
            },
            binding: TranscriptBinding {
                provider: ConversationProvider::Codex,
                data_root: std::path::PathBuf::from("/private"),
                absolute_path: std::path::PathBuf::from("/private/session.jsonl"),
                checkpoint: TranscriptCheckpoint {
                    byte_offset: 10,
                    identity: [7; 32],
                },
            },
            completed: TranscriptCheckpoint {
                byte_offset: 20,
                identity: [7; 32],
            },
        };
        let source_event_id = job.source_event_id();
        let mut state = Self::default();
        state.backends.insert(
            backside_pane_id,
            BackendDelivery {
                lifecycle: BackendLifecycle::Busy,
                role_delivered: true,
                queue: VecDeque::new(),
                in_flight: Some(BackendInFlight::Conversation(job)),
            },
        );
        (state, source_event_id)
    }

    pub(crate) fn persisted(&self) -> PersistedReviewDelivery {
        let backends = self
            .backends
            .iter()
            .map(|(pane_id, backend)| {
                let in_flight = match &backend.in_flight {
                    Some(BackendInFlight::Conversation(job)) => {
                        Some(PersistedDeliveryJob::from(job))
                    }
                    Some(BackendInFlight::Role) | None => None,
                };
                PersistedBackendDelivery {
                    backside_pane_id: pane_id.raw(),
                    queue: backend
                        .queue
                        .iter()
                        .map(PersistedDeliveryJob::from)
                        .collect(),
                    in_flight,
                }
            })
            .collect();
        let fronts = self
            .fronts
            .iter()
            .map(|(pane_id, front)| PersistedFrontDelivery {
                front_pane_id: pane_id.raw(),
                backside_pane_id: front.backside_pane_id.raw(),
                generation: front.generation,
                armed: front.armed,
                phase: front.phase.clone(),
                pending_phases: front.pending_phases.clone(),
                acknowledged_checkpoint: front.acknowledged_checkpoint.clone(),
            })
            .collect();
        PersistedReviewDelivery {
            version: PERSISTED_DELIVERY_VERSION,
            fronts,
            backends,
        }
    }

    pub(crate) fn restore(snapshot: PersistedReviewDelivery) -> Self {
        if snapshot.version != PERSISTED_DELIVERY_VERSION {
            return Self::default();
        }
        let fronts = snapshot
            .fronts
            .into_iter()
            .map(|front| {
                (
                    PaneId::from_raw(front.front_pane_id),
                    FrontDelivery {
                        backside_pane_id: PaneId::from_raw(front.backside_pane_id),
                        generation: front.generation,
                        armed: front.armed,
                        phase: front.phase,
                        pending_phases: front.pending_phases,
                        acknowledged_checkpoint: front.acknowledged_checkpoint,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let backends = snapshot
            .backends
            .into_iter()
            .map(|backend| {
                let mut queue = backend
                    .queue
                    .into_iter()
                    .map(PersistedDeliveryJob::into_job)
                    .collect::<VecDeque<_>>();
                // Delivery uses a deterministic source_event_id, so an
                // uncertain crash-time in-flight item is safe to retry and
                // proposal submission can dedupe it without losing the turn.
                if let Some(in_flight) = backend.in_flight {
                    queue.push_front(in_flight.into_job());
                }
                let lifecycle = if queue.is_empty() {
                    BackendLifecycle::Unassigned
                } else {
                    BackendLifecycle::Failed
                };
                (
                    PaneId::from_raw(backend.backside_pane_id),
                    BackendDelivery {
                        lifecycle,
                        role_delivered: false,
                        queue,
                        in_flight: None,
                    },
                )
            })
            .collect();
        Self { fronts, backends }
    }

    pub(crate) fn resume_actions(&mut self) -> Vec<DeliveryAction> {
        let mut actions = Vec::new();
        let assigned_backends = self
            .fronts
            .values()
            .map(|front| front.backside_pane_id)
            .collect::<Vec<_>>();
        for backside_pane_id in assigned_backends {
            let backend = self.backends.entry(backside_pane_id).or_default();
            match backend.lifecycle {
                BackendLifecycle::Unassigned => backend.lifecycle = BackendLifecycle::Starting,
                BackendLifecycle::Failed => backend.lifecycle = BackendLifecycle::Restarting,
                BackendLifecycle::Starting
                | BackendLifecycle::Ready
                | BackendLifecycle::Busy
                | BackendLifecycle::Restarting => continue,
            }
            backend.role_delivered = false;
            actions.push(DeliveryAction::StartBackend { backside_pane_id });
        }

        for (front_pane_id, front) in &mut self.fronts {
            let mut phases = front.pending_phases.clone();
            phases.push((front.generation, front.phase.clone()));
            for (generation, phase) in phases {
                let assignment = AssignmentIdentity {
                    front_pane_id: *front_pane_id,
                    backside_pane_id: front.backside_pane_id,
                    generation,
                };
                match phase {
                    FrontPhase::Idle => {}
                    FrontPhase::Binding { provider, .. } => {
                        actions.push(DeliveryAction::ResolveTranscript {
                            assignment,
                            provider,
                        });
                    }
                    FrontPhase::Working { binding } => {
                        if generation == front.generation {
                            front.phase = FrontPhase::Probing {
                                binding: binding.clone(),
                            };
                        } else if let Some((_, pending)) = front
                            .pending_phases
                            .iter_mut()
                            .find(|(pending_generation, _)| *pending_generation == generation)
                        {
                            *pending = FrontPhase::Probing {
                                binding: binding.clone(),
                            };
                        }
                        actions.push(DeliveryAction::ProbeCompletion {
                            assignment,
                            binding,
                        });
                    }
                    FrontPhase::Probing { binding } => {
                        actions.push(DeliveryAction::ProbeCompletion {
                            assignment,
                            binding,
                        });
                    }
                }
            }
        }
        actions
    }

    pub(crate) fn retain_assignments(&mut self, pairs: &[(PaneId, PaneId)]) {
        self.fronts
            .retain(|front_id, front| pairs.contains(&(*front_id, front.backside_pane_id)));
        self.backends.retain(|back_id, backend| {
            let assigned = pairs.iter().any(|(_, paired_back)| paired_back == back_id);
            if assigned {
                backend.queue.retain(|job| {
                    pairs.contains(&(
                        job.assignment.front_pane_id,
                        job.assignment.backside_pane_id,
                    ))
                });
            }
            assigned
        });
    }

    pub(crate) fn ensure_assignment(
        &mut self,
        front_pane_id: PaneId,
        backside_pane_id: PaneId,
    ) -> Vec<DeliveryAction> {
        if front_pane_id == backside_pane_id {
            return Vec::new();
        }

        self.fronts
            .entry(front_pane_id)
            .and_modify(|front| front.backside_pane_id = backside_pane_id)
            .or_insert(FrontDelivery {
                backside_pane_id,
                generation: 0,
                armed: false,
                phase: FrontPhase::Idle,
                pending_phases: Vec::new(),
                acknowledged_checkpoint: None,
            });

        let backend = self.backends.entry(backside_pane_id).or_default();
        match backend.lifecycle {
            BackendLifecycle::Unassigned => backend.lifecycle = BackendLifecycle::Starting,
            BackendLifecycle::Failed => backend.lifecycle = BackendLifecycle::Restarting,
            BackendLifecycle::Starting
            | BackendLifecycle::Ready
            | BackendLifecycle::Busy
            | BackendLifecycle::Restarting => return Vec::new(),
        }
        backend.role_delivered = false;
        vec![DeliveryAction::StartBackend { backside_pane_id }]
    }

    pub(crate) fn observe_front_state(
        &mut self,
        front_pane_id: PaneId,
        backside_pane_id: PaneId,
        previous: AgentState,
        current: AgentState,
        agent: Option<Agent>,
    ) -> Vec<DeliveryAction> {
        let Some(provider) = agent.and_then(ConversationProvider::from_agent) else {
            return Vec::new();
        };
        if front_pane_id == backside_pane_id {
            return Vec::new();
        }

        // Detecting a supported front agent assigns and starts its paired
        // Review Agent even when the front is initially idle. Conversation
        // delivery still begins only on a later work transition.
        let mut actions = self.ensure_assignment(front_pane_id, backside_pane_id);

        let Some(front) = self.fronts.get_mut(&front_pane_id) else {
            return actions;
        };
        if !front.armed {
            if current == AgentState::Idle {
                front.armed = true;
            }
            return actions;
        }

        let starts_work = matches!(current, AgentState::Working | AgentState::Blocked)
            && !matches!(previous, AgentState::Working | AgentState::Blocked);
        if starts_work {
            let Some(front) = self.fronts.get_mut(&front_pane_id) else {
                return actions;
            };
            front.backside_pane_id = backside_pane_id;
            if !matches!(front.phase, FrontPhase::Idle) {
                front
                    .pending_phases
                    .push((front.generation, front.phase.clone()));
            }
            front.generation = front.generation.saturating_add(1);
            front.phase = FrontPhase::Binding {
                provider,
                idle_after_bind: false,
            };
            let assignment = AssignmentIdentity {
                front_pane_id,
                backside_pane_id,
                generation: front.generation,
            };
            actions.insert(
                0,
                DeliveryAction::ResolveTranscript {
                    assignment,
                    provider,
                },
            );
            return actions;
        }

        let completes_work = current == AgentState::Idle
            && matches!(previous, AgentState::Working | AgentState::Blocked);
        if !completes_work {
            return actions;
        }
        let Some(front) = self.fronts.get_mut(&front_pane_id) else {
            return actions;
        };
        match &front.phase {
            FrontPhase::Binding { provider, .. } => {
                front.phase = FrontPhase::Binding {
                    provider: *provider,
                    idle_after_bind: true,
                };
                actions
            }
            FrontPhase::Working { binding } => {
                let binding = binding.clone();
                front.phase = FrontPhase::Probing {
                    binding: binding.clone(),
                };
                actions.push(DeliveryAction::ProbeCompletion {
                    assignment: AssignmentIdentity {
                        front_pane_id,
                        backside_pane_id: front.backside_pane_id,
                        generation: front.generation,
                    },
                    binding,
                });
                actions
            }
            FrontPhase::Idle | FrontPhase::Probing { .. } => actions,
        }
    }

    pub(crate) fn transcript_resolved(
        &mut self,
        assignment: &AssignmentIdentity,
        resolution: TranscriptResolution,
    ) -> Vec<DeliveryAction> {
        let Some(phase) = self.matching_phase_mut(assignment) else {
            return Vec::new();
        };
        let idle_after_bind = match *phase {
            FrontPhase::Binding {
                idle_after_bind, ..
            } => idle_after_bind,
            _ => return Vec::new(),
        };
        let TranscriptResolution::Unique(binding) = resolution else {
            *phase = FrontPhase::Idle;
            if let Some(front) = self.fronts.get_mut(&assignment.front_pane_id) {
                if front.generation != assignment.generation {
                    front
                        .pending_phases
                        .retain(|(generation, _)| *generation != assignment.generation);
                }
            }
            return Vec::new();
        };
        if idle_after_bind {
            *phase = FrontPhase::Probing {
                binding: binding.clone(),
            };
            vec![DeliveryAction::ProbeCompletion {
                assignment: assignment.clone(),
                binding,
            }]
        } else {
            *phase = FrontPhase::Working { binding };
            Vec::new()
        }
    }

    pub(crate) fn completion_probed(
        &mut self,
        assignment: &AssignmentIdentity,
        expected_checkpoint: &TranscriptCheckpoint,
        readiness: CompletionReadiness,
    ) -> Vec<DeliveryAction> {
        let Some(phase) = self.matching_phase_mut(assignment) else {
            return Vec::new();
        };
        let binding = match &*phase {
            FrontPhase::Probing { binding } if &binding.checkpoint == expected_checkpoint => {
                binding.clone()
            }
            _ => return Vec::new(),
        };
        let CompletionReadiness::Ready { completed } = readiness else {
            *phase = if readiness == CompletionReadiness::SessionChanged {
                FrontPhase::Idle
            } else {
                FrontPhase::Working { binding }
            };
            return Vec::new();
        };
        *phase = FrontPhase::Idle;
        if let Some(front) = self.fronts.get_mut(&assignment.front_pane_id) {
            if front.generation != assignment.generation {
                front
                    .pending_phases
                    .retain(|(generation, _)| *generation != assignment.generation);
            }
        }
        let job = DeliveryJob {
            assignment: assignment.clone(),
            binding,
            completed,
        };
        let backend = self
            .backends
            .entry(assignment.backside_pane_id)
            .or_default();
        backend.queue.push_back(job);
        Self::next_backend_action(assignment.backside_pane_id, backend)
            .into_iter()
            .collect()
    }

    pub(crate) fn backend_observed(
        &mut self,
        backside_pane_id: PaneId,
        previous: AgentState,
        current: AgentState,
    ) -> Vec<DeliveryAction> {
        let Some(backend) = self.backends.get_mut(&backside_pane_id) else {
            return Vec::new();
        };
        let became_ready = current == AgentState::Idle
            && (matches!(
                backend.lifecycle,
                BackendLifecycle::Starting | BackendLifecycle::Restarting
            ) || (backend.lifecycle == BackendLifecycle::Busy
                && matches!(previous, AgentState::Working | AgentState::Blocked)));
        if !became_ready {
            return Vec::new();
        }

        if let Some(in_flight) = backend.in_flight.take() {
            match in_flight {
                BackendInFlight::Role => backend.role_delivered = true,
                BackendInFlight::Conversation(job) => {
                    if let Some(front) = self.fronts.get_mut(&job.assignment.front_pane_id) {
                        front.acknowledged_checkpoint = Some(job.completed);
                    }
                }
            }
        }
        backend.lifecycle = BackendLifecycle::Ready;
        Self::next_backend_action(backside_pane_id, backend)
            .into_iter()
            .collect()
    }

    pub(crate) fn reconcile_backend_readiness(
        &mut self,
        backside_pane_id: PaneId,
    ) -> Vec<DeliveryAction> {
        let should_reconcile = self.backends.get(&backside_pane_id).is_some_and(|backend| {
            matches!(
                backend.lifecycle,
                BackendLifecycle::Starting | BackendLifecycle::Restarting
            )
        });
        if !should_reconcile {
            return Vec::new();
        }
        self.backend_observed(backside_pane_id, AgentState::Unknown, AgentState::Idle)
    }

    pub(crate) fn backend_awaiting_readiness(&self, backside_pane_id: PaneId) -> bool {
        self.backends.get(&backside_pane_id).is_some_and(|backend| {
            matches!(
                backend.lifecycle,
                BackendLifecycle::Starting | BackendLifecycle::Restarting
            )
        })
    }

    pub(crate) fn backend_send_succeeded(&mut self, backside_pane_id: PaneId) {
        if let Some(backend) = self.backends.get_mut(&backside_pane_id) {
            if backend.in_flight.is_some() && backend.lifecycle == BackendLifecycle::Ready {
                backend.lifecycle = BackendLifecycle::Busy;
            }
        }
    }

    pub(crate) fn backend_send_failed(&mut self, backside_pane_id: PaneId) -> Vec<DeliveryAction> {
        let Some(backend) = self.backends.get_mut(&backside_pane_id) else {
            return Vec::new();
        };
        if let Some(BackendInFlight::Conversation(job)) = backend.in_flight.take() {
            backend.queue.push_front(job);
        } else {
            backend.in_flight = None;
        }
        backend.lifecycle = BackendLifecycle::Failed;
        Vec::new()
    }

    pub(crate) fn restart_backend(&mut self, backside_pane_id: PaneId) -> Vec<DeliveryAction> {
        let Some(backend) = self.backends.get_mut(&backside_pane_id) else {
            return Vec::new();
        };
        if backend.lifecycle != BackendLifecycle::Failed {
            return Vec::new();
        }
        backend.lifecycle = BackendLifecycle::Restarting;
        backend.role_delivered = false;
        vec![DeliveryAction::StartBackend { backside_pane_id }]
    }

    pub(crate) fn restart_backend_after_exit(
        &mut self,
        backside_pane_id: PaneId,
    ) -> Vec<DeliveryAction> {
        let Some(backend) = self.backends.get_mut(&backside_pane_id) else {
            return Vec::new();
        };
        if backend.lifecycle != BackendLifecycle::Failed {
            return Vec::new();
        }
        backend.lifecycle = BackendLifecycle::Restarting;
        backend.role_delivered = false;
        vec![DeliveryAction::RestartBackendAfterExit { backside_pane_id }]
    }

    pub(crate) fn backend_died(&mut self, backside_pane_id: PaneId) {
        if let Some(backend) = self.backends.get_mut(&backside_pane_id) {
            if let Some(BackendInFlight::Conversation(job)) = backend.in_flight.take() {
                backend.queue.push_front(job);
            } else {
                backend.in_flight = None;
            }
            backend.lifecycle = BackendLifecycle::Failed;
        }
    }

    pub(crate) fn prepare_backend_replacement(&mut self, backside_pane_id: PaneId) {
        let backend = self.backends.entry(backside_pane_id).or_default();
        if let Some(BackendInFlight::Conversation(job)) = backend.in_flight.take() {
            backend.queue.push_front(job);
        } else {
            // A role sent to the runtime being replaced cannot acknowledge
            // initialization for the fresh backend.
            backend.in_flight = None;
        }
        backend.lifecycle = BackendLifecycle::Starting;
        backend.role_delivered = false;
    }

    pub(crate) fn remove_front(&mut self, front_pane_id: PaneId) {
        let Some(front) = self.fronts.remove(&front_pane_id) else {
            return;
        };
        if let Some(backend) = self.backends.get_mut(&front.backside_pane_id) {
            backend
                .queue
                .retain(|job| job.assignment.front_pane_id != front_pane_id);
            if matches!(
                &backend.in_flight,
                Some(BackendInFlight::Conversation(job))
                    if job.assignment.front_pane_id == front_pane_id
            ) {
                backend.in_flight = None;
                backend.lifecycle = BackendLifecycle::Failed;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn backend_lifecycle(&self, backside_pane_id: PaneId) -> BackendLifecycle {
        self.backends
            .get(&backside_pane_id)
            .map_or(BackendLifecycle::Unassigned, |backend| backend.lifecycle)
    }

    fn matching_phase_mut(&mut self, assignment: &AssignmentIdentity) -> Option<&mut FrontPhase> {
        let front = self.fronts.get_mut(&assignment.front_pane_id)?;
        if front.backside_pane_id != assignment.backside_pane_id {
            return None;
        }
        if front.generation == assignment.generation {
            return Some(&mut front.phase);
        }
        front
            .pending_phases
            .iter_mut()
            .find(|(generation, _)| *generation == assignment.generation)
            .map(|(_, phase)| phase)
    }

    fn next_backend_action(
        backside_pane_id: PaneId,
        backend: &mut BackendDelivery,
    ) -> Option<DeliveryAction> {
        if backend.lifecycle != BackendLifecycle::Ready || backend.in_flight.is_some() {
            return None;
        }
        if !backend.role_delivered {
            backend.in_flight = Some(BackendInFlight::Role);
            backend.lifecycle = BackendLifecycle::Busy;
            return Some(DeliveryAction::SendRole { backside_pane_id });
        }
        let job = backend.queue.pop_front()?;
        backend.in_flight = Some(BackendInFlight::Conversation(job.clone()));
        backend.lifecycle = BackendLifecycle::Busy;
        Some(DeliveryAction::SendConversation(job))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn binding(provider: ConversationProvider, offset: u64) -> TranscriptBinding {
        TranscriptBinding {
            provider,
            data_root: PathBuf::from("/private"),
            absolute_path: PathBuf::from(format!("/private/session-{offset}.jsonl")),
            checkpoint: TranscriptCheckpoint {
                byte_offset: offset,
                identity: [offset as u8; 32],
            },
        }
    }

    fn start_generation(
        state: &mut ReviewDeliveryState,
        front: PaneId,
        back: PaneId,
    ) -> AssignmentIdentity {
        state.observe_front_state(
            front,
            back,
            AgentState::Unknown,
            AgentState::Idle,
            Some(Agent::Codex),
        );
        let actions = state.observe_front_state(
            front,
            back,
            AgentState::Idle,
            AgentState::Working,
            Some(Agent::Codex),
        );
        actions
            .into_iter()
            .find_map(|action| match action {
                DeliveryAction::ResolveTranscript { assignment, .. } => Some(assignment),
                _ => None,
            })
            .expect("resolve action")
    }

    #[test]
    fn initial_idle_starts_backend_once_without_delivering_conversation() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        assert!(matches!(
            state
                .observe_front_state(
                front,
                back,
                AgentState::Unknown,
                AgentState::Idle,
                Some(Agent::Codex),
            )
                .as_slice(),
            [DeliveryAction::StartBackend { backside_pane_id }] if *backside_pane_id == back
        ));
        assert!(state
            .observe_front_state(
                front,
                back,
                AgentState::Idle,
                AgentState::Idle,
                Some(Agent::Codex),
            )
            .is_empty());
    }

    #[test]
    fn startup_working_is_ignored_until_idle_arms_persisted_front() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();

        let startup = state.observe_front_state(
            front,
            back,
            AgentState::Unknown,
            AgentState::Working,
            Some(Agent::Codex),
        );
        assert!(matches!(
            startup.as_slice(),
            [DeliveryAction::StartBackend { backside_pane_id }] if *backside_pane_id == back
        ));
        assert!(state
            .observe_front_state(
                front,
                back,
                AgentState::Working,
                AgentState::Idle,
                Some(Agent::Codex),
            )
            .is_empty());

        let mut restored = ReviewDeliveryState::restore(state.persisted());
        let first_real_turn = restored.observe_front_state(
            front,
            back,
            AgentState::Idle,
            AgentState::Working,
            Some(Agent::Codex),
        );
        assert!(first_real_turn.iter().any(|action| matches!(
            action,
            DeliveryAction::ResolveTranscript {
                assignment,
                provider: ConversationProvider::Codex,
            } if assignment.front_pane_id == front && assignment.backside_pane_id == back
        )));
    }

    #[test]
    fn ensured_assignment_does_not_start_same_backend_twice() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();

        assert!(matches!(
            state.ensure_assignment(front, back).as_slice(),
            [DeliveryAction::StartBackend { backside_pane_id }] if *backside_pane_id == back
        ));
        assert!(state.ensure_assignment(front, back).is_empty());
    }

    #[test]
    fn idle_before_resolution_probes_after_binding_arrives() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let assignment = start_generation(&mut state, front, back);
        assert!(state
            .observe_front_state(
                front,
                back,
                AgentState::Working,
                AgentState::Idle,
                Some(Agent::Codex),
            )
            .is_empty());
        assert!(matches!(
            state
                .transcript_resolved(
                    &assignment,
                    TranscriptResolution::Unique(binding(ConversationProvider::Codex, 10)),
                )
                .as_slice(),
            [DeliveryAction::ProbeCompletion { .. }]
        ));
    }

    #[test]
    fn next_working_generation_keeps_completed_pending_resolution() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let stale = start_generation(&mut state, front, back);
        state.observe_front_state(
            front,
            back,
            AgentState::Working,
            AgentState::Idle,
            Some(Agent::Codex),
        );
        let current = state
            .observe_front_state(
                front,
                back,
                AgentState::Idle,
                AgentState::Working,
                Some(Agent::Codex),
            )
            .into_iter()
            .find_map(|action| match action {
                DeliveryAction::ResolveTranscript { assignment, .. } => Some(assignment),
                _ => None,
            })
            .expect("new generation");
        assert!(current.generation > stale.generation);
        assert!(matches!(
            state
                .transcript_resolved(
                    &stale,
                    TranscriptResolution::Unique(binding(ConversationProvider::Codex, 1)),
                )
                .as_slice(),
            [DeliveryAction::ProbeCompletion { assignment, .. }]
                if assignment.generation == stale.generation
        ));
    }

    #[test]
    fn backend_busy_keeps_completed_turns_fifo() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let first = start_generation(&mut state, front, back);
        let first_binding = binding(ConversationProvider::Codex, 10);
        state.transcript_resolved(&first, TranscriptResolution::Unique(first_binding.clone()));
        state.observe_front_state(
            front,
            back,
            AgentState::Working,
            AgentState::Idle,
            Some(Agent::Codex),
        );
        state.completion_probed(
            &first,
            &first_binding.checkpoint,
            CompletionReadiness::Ready {
                completed: TranscriptCheckpoint {
                    byte_offset: 20,
                    identity: first_binding.checkpoint.identity,
                },
            },
        );

        let second = start_generation(&mut state, front, back);
        let second_binding = binding(ConversationProvider::Codex, 20);
        state.transcript_resolved(
            &second,
            TranscriptResolution::Unique(second_binding.clone()),
        );
        state.observe_front_state(
            front,
            back,
            AgentState::Working,
            AgentState::Idle,
            Some(Agent::Codex),
        );
        state.completion_probed(
            &second,
            &second_binding.checkpoint,
            CompletionReadiness::Ready {
                completed: TranscriptCheckpoint {
                    byte_offset: 30,
                    identity: second_binding.checkpoint.identity,
                },
            },
        );

        let role = state.backend_observed(back, AgentState::Working, AgentState::Idle);
        assert!(matches!(role.as_slice(), [DeliveryAction::SendRole { .. }]));
        state.backend_send_succeeded(back);
        let first_send = state.backend_observed(back, AgentState::Working, AgentState::Idle);
        assert!(matches!(
            first_send.as_slice(),
            [DeliveryAction::SendConversation(job)] if job.completed.byte_offset == 20
        ));
        state.backend_send_succeeded(back);
        let second_send = state.backend_observed(back, AgentState::Working, AgentState::Idle);
        assert!(matches!(
            second_send.as_slice(),
            [DeliveryAction::SendConversation(job)] if job.completed.byte_offset == 30
        ));
    }

    #[test]
    fn send_failure_requeues_and_restart_redelivers_role_first() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let assignment = start_generation(&mut state, front, back);
        let transcript = binding(ConversationProvider::Claude, 10);
        state.transcript_resolved(
            &assignment,
            TranscriptResolution::Unique(transcript.clone()),
        );
        state.observe_front_state(
            front,
            back,
            AgentState::Working,
            AgentState::Idle,
            Some(Agent::Claude),
        );
        state.completion_probed(
            &assignment,
            &transcript.checkpoint,
            CompletionReadiness::Ready {
                completed: TranscriptCheckpoint {
                    byte_offset: 20,
                    identity: transcript.checkpoint.identity,
                },
            },
        );
        state.backend_observed(back, AgentState::Working, AgentState::Idle);
        state.backend_send_succeeded(back);
        state.backend_observed(back, AgentState::Working, AgentState::Idle);
        state.backend_send_failed(back);
        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Failed);
        assert!(matches!(
            state.restart_backend(back).as_slice(),
            [DeliveryAction::StartBackend { .. }]
        ));
        assert!(matches!(
            state
                .backend_observed(back, AgentState::Working, AgentState::Idle)
                .as_slice(),
            [DeliveryAction::SendRole { .. }]
        ));
    }

    #[test]
    fn replacement_drops_role_sent_to_old_runtime_and_roles_fresh_backend() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        state.ensure_assignment(front, back);

        assert!(matches!(
            state
                .backend_observed(back, AgentState::Unknown, AgentState::Idle)
                .as_slice(),
            [DeliveryAction::SendRole { backside_pane_id }] if *backside_pane_id == back
        ));
        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Busy);

        state.prepare_backend_replacement(back);

        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Starting);
        assert!(matches!(
            state
                .backend_observed(back, AgentState::Unknown, AgentState::Idle)
                .as_slice(),
            [DeliveryAction::SendRole { backside_pane_id }] if *backside_pane_id == back
        ));
    }

    #[test]
    fn replacement_requeues_conversation_before_fresh_role() {
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let job = DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id: front,
                backside_pane_id: back,
                generation: 1,
            },
            binding: binding(ConversationProvider::Codex, 10),
            completed: TranscriptCheckpoint {
                byte_offset: 20,
                identity: [10; 32],
            },
        };
        let mut state = ReviewDeliveryState::default();
        state.backends.insert(
            back,
            BackendDelivery {
                lifecycle: BackendLifecycle::Busy,
                role_delivered: true,
                queue: VecDeque::new(),
                in_flight: Some(BackendInFlight::Conversation(job.clone())),
            },
        );

        state.prepare_backend_replacement(back);
        assert!(matches!(
            state
                .backend_observed(back, AgentState::Unknown, AgentState::Idle)
                .as_slice(),
            [DeliveryAction::SendRole { .. }]
        ));
        state.backend_send_succeeded(back);
        assert!(matches!(
            state
                .backend_observed(back, AgentState::Working, AgentState::Idle)
                .as_slice(),
            [DeliveryAction::SendConversation(requeued)] if *requeued == job
        ));
    }

    #[test]
    fn readiness_reconciliation_does_not_ack_busy_conversation() {
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let job = DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id: front,
                backside_pane_id: back,
                generation: 1,
            },
            binding: binding(ConversationProvider::Codex, 10),
            completed: TranscriptCheckpoint {
                byte_offset: 20,
                identity: [10; 32],
            },
        };
        let mut state = ReviewDeliveryState::default();
        state.backends.insert(
            back,
            BackendDelivery {
                lifecycle: BackendLifecycle::Busy,
                role_delivered: true,
                queue: VecDeque::new(),
                in_flight: Some(BackendInFlight::Conversation(job.clone())),
            },
        );

        assert!(state.reconcile_backend_readiness(back).is_empty());
        assert!(matches!(
            state.backends[&back].in_flight.as_ref(),
            Some(BackendInFlight::Conversation(in_flight)) if *in_flight == job
        ));
        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Busy);

        assert!(state
            .backend_observed(back, AgentState::Working, AgentState::Idle)
            .is_empty());
        assert!(state.backends[&back].in_flight.is_none());
        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Ready);
    }

    #[test]
    fn backside_cannot_be_its_own_front() {
        let mut state = ReviewDeliveryState::default();
        let pane = PaneId::alloc();
        assert!(state
            .observe_front_state(
                pane,
                pane,
                AgentState::Idle,
                AgentState::Working,
                Some(Agent::Codex),
            )
            .is_empty());
    }

    #[test]
    fn backend_death_marks_startup_failed_and_can_restart() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        start_generation(&mut state, front, back);
        state.backend_died(back);
        assert_eq!(state.backend_lifecycle(back), BackendLifecycle::Failed);
        assert!(matches!(
            state.restart_backend(back).as_slice(),
            [DeliveryAction::StartBackend { .. }]
        ));
    }

    #[test]
    fn removing_front_cancels_its_queued_work() {
        let mut state = ReviewDeliveryState::default();
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        start_generation(&mut state, front, back);
        state.remove_front(front);
        assert!(state
            .observe_front_state(
                front,
                back,
                AgentState::Unknown,
                AgentState::Idle,
                Some(Agent::Codex),
            )
            .is_empty());
    }

    #[test]
    fn persistence_requeues_uncertain_in_flight_before_pending_delivery() {
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let pending = DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id: front,
                backside_pane_id: back,
                generation: 2,
            },
            binding: binding(ConversationProvider::Codex, 20),
            completed: TranscriptCheckpoint {
                byte_offset: 30,
                identity: [20; 32],
            },
        };
        let uncertain = DeliveryJob {
            assignment: AssignmentIdentity {
                front_pane_id: front,
                backside_pane_id: back,
                generation: 1,
            },
            binding: binding(ConversationProvider::Codex, 10),
            completed: TranscriptCheckpoint {
                byte_offset: 20,
                identity: [10; 32],
            },
        };
        let mut state = ReviewDeliveryState::default();
        state.fronts.insert(
            front,
            FrontDelivery {
                backside_pane_id: back,
                generation: 2,
                armed: true,
                phase: FrontPhase::Idle,
                pending_phases: Vec::new(),
                acknowledged_checkpoint: None,
            },
        );
        state.backends.insert(
            back,
            BackendDelivery {
                lifecycle: BackendLifecycle::Busy,
                role_delivered: true,
                queue: VecDeque::from([pending]),
                in_flight: Some(BackendInFlight::Conversation(uncertain)),
            },
        );

        let mut restored = ReviewDeliveryState::restore(state.persisted());
        assert_eq!(
            restored.fronts[&front]
                .acknowledged_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.byte_offset),
            None
        );
        assert_eq!(restored.backends[&back].queue.len(), 2);
        assert_eq!(restored.backends[&back].queue[0].completed.byte_offset, 20);
        assert_eq!(restored.backends[&back].queue[1].completed.byte_offset, 30);
        assert!(restored.backends[&back].in_flight.is_none());
        assert!(matches!(
            restored.resume_actions().as_slice(),
            [DeliveryAction::StartBackend { backside_pane_id }] if *backside_pane_id == back
        ));
    }

    #[test]
    fn restored_binding_reissues_provider_specific_resolution() {
        let front = PaneId::alloc();
        let back = PaneId::alloc();
        let mut state = ReviewDeliveryState::default();
        state.fronts.insert(
            front,
            FrontDelivery {
                backside_pane_id: back,
                generation: 4,
                armed: true,
                phase: FrontPhase::Binding {
                    provider: ConversationProvider::Claude,
                    idle_after_bind: true,
                },
                pending_phases: Vec::new(),
                acknowledged_checkpoint: None,
            },
        );

        let mut restored = ReviewDeliveryState::restore(state.persisted());
        let actions = restored.resume_actions();
        assert!(actions.iter().any(|action| matches!(
            action,
            DeliveryAction::ResolveTranscript {
                assignment,
                provider: ConversationProvider::Claude,
            } if assignment.front_pane_id == front
                && assignment.backside_pane_id == back
                && assignment.generation == 4
        )));
    }

    #[test]
    fn restored_working_and_probing_generations_reach_delivery() {
        for phase in [
            FrontPhase::Working {
                binding: binding(ConversationProvider::Codex, 10),
            },
            FrontPhase::Probing {
                binding: binding(ConversationProvider::Codex, 10),
            },
        ] {
            let front = PaneId::alloc();
            let back = PaneId::alloc();
            let mut state = ReviewDeliveryState::default();
            state.fronts.insert(
                front,
                FrontDelivery {
                    backside_pane_id: back,
                    generation: 3,
                    armed: true,
                    phase,
                    pending_phases: Vec::new(),
                    acknowledged_checkpoint: None,
                },
            );

            let mut restored = ReviewDeliveryState::restore(state.persisted());
            let actions = restored.resume_actions();
            assert!(actions.iter().any(|action| matches!(
                action,
                DeliveryAction::StartBackend { backside_pane_id } if *backside_pane_id == back
            )));
            let (assignment, binding) = actions
                .into_iter()
                .find_map(|action| match action {
                    DeliveryAction::ProbeCompletion {
                        assignment,
                        binding,
                    } => Some((assignment, binding)),
                    _ => None,
                })
                .expect("restored completion probe");
            assert!(restored
                .completion_probed(
                    &assignment,
                    &binding.checkpoint,
                    CompletionReadiness::Ready {
                        completed: TranscriptCheckpoint {
                            byte_offset: 20,
                            identity: binding.checkpoint.identity,
                        },
                    },
                )
                .is_empty());

            assert!(matches!(
                restored
                    .backend_observed(back, AgentState::Unknown, AgentState::Idle)
                    .as_slice(),
                [DeliveryAction::SendRole { backside_pane_id }] if *backside_pane_id == back
            ));
            assert!(matches!(
                restored
                    .backend_observed(back, AgentState::Working, AgentState::Idle)
                    .as_slice(),
                [DeliveryAction::SendConversation(job)]
                    if job.assignment == assignment && job.completed.byte_offset == 20
            ));
        }
    }
}
