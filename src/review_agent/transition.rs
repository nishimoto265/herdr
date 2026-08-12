use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    ActiveRule, ReviewBackendProfileId, RuleProposal, RuleProposalChange, RuleProposalDecision,
    RuleProposalDecisionRequest, RuleProposalId, RuleProposalStatus, RuleProposalSubmission,
    RuleProposalSubmitInput, RuleProposalSubmitOutcome, MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT,
    PROPOSAL_EVIDENCE_THRESHOLD,
};

const MAX_RULE_TEXT_BYTES: usize = 16 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_EVIDENCE_CANDIDATES: usize = 256;
const MAX_RULE_PROPOSALS: usize = 1024;
type ScopedByFingerprint<T> = BTreeMap<ReviewBackendProfileId, BTreeMap<String, T>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RuleEvidence {
    rule_text: String,
    target_profile_id: ReviewBackendProfileId,
    source_event_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewAgentState {
    proposals: BTreeMap<RuleProposalId, RuleProposal>,
    proposal_by_fingerprint: ScopedByFingerprint<RuleProposalId>,
    evidence_by_fingerprint: ScopedByFingerprint<RuleEvidence>,
    active_rules: ScopedByFingerprint<ActiveRule>,
    next_proposal_sequence: u64,
}

impl Default for ReviewAgentState {
    fn default() -> Self {
        Self {
            proposals: BTreeMap::new(),
            proposal_by_fingerprint: BTreeMap::new(),
            evidence_by_fingerprint: BTreeMap::new(),
            active_rules: BTreeMap::new(),
            next_proposal_sequence: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubmitTransition {
    pub submission: RuleProposalSubmission,
    pub changed: bool,
    pub proposal_change: Option<RuleProposalChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubmitError {
    EmptyField(&'static str),
    FieldTooLong(&'static str),
    FingerprintConflict,
    InvalidControlCharacter(&'static str),
    LimitExceeded(&'static str),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "{field} must not be empty"),
            Self::FieldTooLong(field) => write!(f, "{field} is too long"),
            Self::FingerprintConflict => {
                write!(
                    f,
                    "fingerprint was already observed with different rule data"
                )
            }
            Self::InvalidControlCharacter(field) => {
                write!(f, "{field} contains an unsupported control character")
            }
            Self::LimitExceeded(limit) => write!(f, "{limit} limit exceeded"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionTransition {
    pub proposal: RuleProposal,
    pub changed: bool,
    pub proposal_change: Option<RuleProposalChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionError {
    ProposalNotFound,
    RevisionConflict { current_revision: u64 },
    AlreadyDecided { status: RuleProposalStatus },
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProposalNotFound => write!(f, "rule proposal was not found"),
            Self::RevisionConflict { current_revision } => {
                write!(
                    f,
                    "proposal revision conflict; current revision is {current_revision}"
                )
            }
            Self::AlreadyDecided { status } => {
                write!(f, "proposal already has status {status:?}")
            }
        }
    }
}

impl ReviewAgentState {
    pub(crate) fn proposals(&self) -> impl Iterator<Item = &RuleProposal> {
        self.proposals.values()
    }

    pub(crate) fn active_rules(&self) -> impl Iterator<Item = &ActiveRule> {
        self.active_rules.values().flat_map(BTreeMap::values)
    }

    pub(crate) fn submit(
        &mut self,
        input: RuleProposalSubmitInput,
    ) -> Result<SubmitTransition, SubmitError> {
        validate_submit_input(&input)?;

        let target_profile_id = input.target_profile_id.clone();
        let fingerprint = input.fingerprint.clone();

        if let Some(proposal_id) = scoped_get(
            &self.proposal_by_fingerprint,
            &target_profile_id,
            &fingerprint,
        )
        .cloned()
        {
            if let Some(proposal) = self.proposals.get_mut(&proposal_id) {
                if proposal.rule_text != input.rule_text
                    || proposal.target_profile_id != input.target_profile_id
                {
                    return Err(SubmitError::FingerprintConflict);
                }
                if proposal
                    .source_event_ids
                    .iter()
                    .any(|event_id| event_id == &input.source_event_id)
                {
                    return Ok(existing_submission(
                        proposal,
                        RuleProposalSubmitOutcome::DuplicateEvent,
                    ));
                }
                match proposal.status {
                    RuleProposalStatus::Approved => {
                        return Ok(existing_submission(
                            proposal,
                            RuleProposalSubmitOutcome::SuppressedApproved,
                        ));
                    }
                    RuleProposalStatus::Rejected => {
                        return Ok(existing_submission(
                            proposal,
                            RuleProposalSubmitOutcome::SuppressedRejected,
                        ));
                    }
                    RuleProposalStatus::Pending => {
                        return Ok(SubmitTransition {
                            submission: RuleProposalSubmission {
                                outcome: RuleProposalSubmitOutcome::SuppressedPending,
                                evidence_count: proposal.source_event_ids.len(),
                                proposal: Some(proposal.clone()),
                            },
                            changed: false,
                            proposal_change: None,
                        });
                    }
                }
            } else {
                // A malformed persisted index must not make submissions panic.
                scoped_remove(
                    &mut self.proposal_by_fingerprint,
                    &target_profile_id,
                    &fingerprint,
                );
            }
        }

        if scoped_get(
            &self.evidence_by_fingerprint,
            &target_profile_id,
            &fingerprint,
        )
        .is_none()
            && scoped_len(&self.evidence_by_fingerprint) >= MAX_EVIDENCE_CANDIDATES
        {
            return Err(SubmitError::LimitExceeded("rule evidence candidate"));
        }

        let source_event_already_recorded = scoped_get(
            &self.evidence_by_fingerprint,
            &target_profile_id,
            &fingerprint,
        )
        .is_some_and(|evidence| evidence.source_event_ids.contains(&input.source_event_id));
        if !source_event_already_recorded
            && self.source_event_observation_count(&input.source_event_id)
                >= MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT
        {
            return Err(SubmitError::LimitExceeded("source event rule observation"));
        }

        let evidence = self
            .evidence_by_fingerprint
            .entry(target_profile_id.clone())
            .or_default()
            .entry(fingerprint.clone())
            .or_insert_with(|| RuleEvidence {
                rule_text: input.rule_text.clone(),
                target_profile_id: input.target_profile_id.clone(),
                source_event_ids: BTreeSet::new(),
            });
        if evidence.rule_text != input.rule_text
            || evidence.target_profile_id != input.target_profile_id
        {
            return Err(SubmitError::FingerprintConflict);
        }
        if evidence.source_event_ids.contains(&input.source_event_id) {
            return Ok(SubmitTransition {
                submission: RuleProposalSubmission {
                    outcome: RuleProposalSubmitOutcome::DuplicateEvent,
                    evidence_count: evidence.source_event_ids.len(),
                    proposal: None,
                },
                changed: false,
                proposal_change: None,
            });
        }
        if evidence.source_event_ids.len() + 1 >= PROPOSAL_EVIDENCE_THRESHOLD
            && self.proposals.len() >= MAX_RULE_PROPOSALS
        {
            return Err(SubmitError::LimitExceeded("rule proposal"));
        }
        evidence.source_event_ids.insert(input.source_event_id);

        let evidence_count = evidence.source_event_ids.len();
        if evidence_count < PROPOSAL_EVIDENCE_THRESHOLD {
            return Ok(SubmitTransition {
                submission: RuleProposalSubmission {
                    outcome: RuleProposalSubmitOutcome::EvidenceRecorded,
                    evidence_count,
                    proposal: None,
                },
                changed: true,
                proposal_change: None,
            });
        }

        let proposal_id =
            RuleProposalId::new(format!("rule-proposal-{}", self.next_proposal_sequence));
        self.next_proposal_sequence = self.next_proposal_sequence.saturating_add(1);
        let proposal = RuleProposal {
            proposal_id: proposal_id.clone(),
            rule_text: evidence.rule_text.clone(),
            target_profile_id: evidence.target_profile_id.clone(),
            fingerprint: input.fingerprint.clone(),
            source_event_ids: evidence.source_event_ids.iter().cloned().collect(),
            status: RuleProposalStatus::Pending,
            revision: 1,
        };
        scoped_insert(
            &mut self.proposal_by_fingerprint,
            target_profile_id,
            input.fingerprint,
            proposal_id.clone(),
        );
        self.proposals.insert(proposal_id, proposal.clone());
        scoped_remove(
            &mut self.evidence_by_fingerprint,
            &proposal.target_profile_id,
            &proposal.fingerprint,
        );

        Ok(SubmitTransition {
            submission: RuleProposalSubmission {
                outcome: RuleProposalSubmitOutcome::ProposalCreated,
                evidence_count,
                proposal: Some(proposal),
            },
            changed: true,
            proposal_change: Some(RuleProposalChange::Proposed),
        })
    }

    pub(crate) fn decide(
        &mut self,
        request: RuleProposalDecisionRequest,
    ) -> Result<DecisionTransition, DecisionError> {
        let Some(proposal) = self.proposals.get_mut(&request.proposal_id) else {
            return Err(DecisionError::ProposalNotFound);
        };
        let requested_status = match request.decision {
            RuleProposalDecision::Approve => RuleProposalStatus::Approved,
            RuleProposalDecision::Reject => RuleProposalStatus::Rejected,
        };
        if proposal.status == requested_status {
            return Ok(DecisionTransition {
                proposal: proposal.clone(),
                changed: false,
                proposal_change: None,
            });
        }
        if proposal.status != RuleProposalStatus::Pending {
            return Err(DecisionError::AlreadyDecided {
                status: proposal.status,
            });
        }
        if proposal.revision != request.expected_revision {
            return Err(DecisionError::RevisionConflict {
                current_revision: proposal.revision,
            });
        }

        proposal.status = requested_status;
        proposal.revision = proposal.revision.saturating_add(1);
        let proposal = proposal.clone();
        let proposal_change = match request.decision {
            RuleProposalDecision::Approve => {
                scoped_insert(
                    &mut self.active_rules,
                    proposal.target_profile_id.clone(),
                    proposal.fingerprint.clone(),
                    ActiveRule {
                        proposal_id: proposal.proposal_id.clone(),
                        rule_text: proposal.rule_text.clone(),
                        target_profile_id: proposal.target_profile_id.clone(),
                        fingerprint: proposal.fingerprint.clone(),
                    },
                );
                RuleProposalChange::Approved
            }
            RuleProposalDecision::Reject => RuleProposalChange::Rejected,
        };

        Ok(DecisionTransition {
            proposal,
            changed: true,
            proposal_change: Some(proposal_change),
        })
    }

    fn source_event_observation_count(&self, source_event_id: &str) -> usize {
        let evidence_count = self
            .evidence_by_fingerprint
            .values()
            .flat_map(BTreeMap::values)
            .filter(|evidence| evidence.source_event_ids.contains(source_event_id))
            .count();
        let proposal_count = self
            .proposals
            .values()
            .filter(|proposal| {
                proposal
                    .source_event_ids
                    .iter()
                    .any(|stored| stored == source_event_id)
            })
            .count();
        evidence_count.saturating_add(proposal_count)
    }
}

fn scoped_get<'a, T>(
    values: &'a ScopedByFingerprint<T>,
    profile_id: &ReviewBackendProfileId,
    fingerprint: &str,
) -> Option<&'a T> {
    values
        .get(profile_id)
        .and_then(|profile_values| profile_values.get(fingerprint))
}

fn scoped_insert<T>(
    values: &mut ScopedByFingerprint<T>,
    profile_id: ReviewBackendProfileId,
    fingerprint: String,
    value: T,
) {
    values
        .entry(profile_id)
        .or_default()
        .insert(fingerprint, value);
}

fn scoped_remove<T>(
    values: &mut ScopedByFingerprint<T>,
    profile_id: &ReviewBackendProfileId,
    fingerprint: &str,
) {
    let should_remove_profile = values.get_mut(profile_id).is_some_and(|profile_values| {
        profile_values.remove(fingerprint);
        profile_values.is_empty()
    });
    if should_remove_profile {
        values.remove(profile_id);
    }
}

fn scoped_len<T>(values: &ScopedByFingerprint<T>) -> usize {
    values.values().map(BTreeMap::len).sum()
}

fn existing_submission(
    proposal: &RuleProposal,
    outcome: RuleProposalSubmitOutcome,
) -> SubmitTransition {
    SubmitTransition {
        submission: RuleProposalSubmission {
            outcome,
            evidence_count: proposal.source_event_ids.len(),
            proposal: Some(proposal.clone()),
        },
        changed: false,
        proposal_change: None,
    }
}

fn validate_submit_input(input: &RuleProposalSubmitInput) -> Result<(), SubmitError> {
    validate_field("rule_text", &input.rule_text, MAX_RULE_TEXT_BYTES)?;
    if input
        .rule_text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(SubmitError::InvalidControlCharacter("rule_text"));
    }
    validate_field(
        "target_profile_id",
        input.target_profile_id.as_str(),
        MAX_IDENTIFIER_BYTES,
    )?;
    validate_field("fingerprint", &input.fingerprint, MAX_IDENTIFIER_BYTES)?;
    validate_field(
        "source_event_id",
        &input.source_event_id,
        MAX_IDENTIFIER_BYTES,
    )
}

fn validate_field(field: &'static str, value: &str, max_bytes: usize) -> Result<(), SubmitError> {
    if value.trim().is_empty() {
        return Err(SubmitError::EmptyField(field));
    }
    if value.len() > max_bytes {
        return Err(SubmitError::FieldTooLong(field));
    }
    if field != "rule_text" && value.chars().any(char::is_control) {
        return Err(SubmitError::InvalidControlCharacter(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event: &str) -> RuleProposalSubmitInput {
        RuleProposalSubmitInput {
            rule_text: "Review callers affected by changed behavior.".into(),
            target_profile_id: ReviewBackendProfileId::new("review-agent"),
            fingerprint: "review-callers".into(),
            source_event_id: event.into(),
        }
    }

    fn pending_proposal(state: &mut ReviewAgentState) -> RuleProposal {
        state.submit(input("completion-1")).unwrap();
        state
            .submit(input("completion-2"))
            .unwrap()
            .submission
            .proposal
            .expect("second distinct event should create proposal")
    }

    #[test]
    fn duplicate_event_is_idempotent() {
        let mut state = ReviewAgentState::default();
        let first = state.submit(input("completion-1")).unwrap();
        let duplicate = state.submit(input("completion-1")).unwrap();

        assert_eq!(first.submission.evidence_count, 1);
        assert_eq!(
            duplicate.submission.outcome,
            RuleProposalSubmitOutcome::DuplicateEvent
        );
        assert!(!duplicate.changed);
        assert!(state.proposals().next().is_none());
    }

    #[test]
    fn two_distinct_events_create_one_pending_proposal() {
        let mut state = ReviewAgentState::default();
        state.submit(input("completion-1")).unwrap();
        let second = state.submit(input("completion-2")).unwrap();

        assert_eq!(
            second.submission.outcome,
            RuleProposalSubmitOutcome::ProposalCreated
        );
        let proposals = state.proposals().collect::<Vec<_>>();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].status, RuleProposalStatus::Pending);
        assert_eq!(proposals[0].source_event_ids.len(), 2);
    }

    #[test]
    fn rejected_fingerprint_is_suppressed() {
        let mut state = ReviewAgentState::default();
        let proposal = pending_proposal(&mut state);
        state
            .decide(RuleProposalDecisionRequest {
                proposal_id: proposal.proposal_id,
                expected_revision: proposal.revision,
                decision: RuleProposalDecision::Reject,
            })
            .unwrap();

        let result = state.submit(input("completion-3")).unwrap();
        assert_eq!(
            result.submission.outcome,
            RuleProposalSubmitOutcome::SuppressedRejected
        );
        assert!(!result.changed);
        assert_eq!(state.proposals().count(), 1);
    }

    #[test]
    fn approval_adds_rule_to_active_set() {
        let mut state = ReviewAgentState::default();
        let proposal = pending_proposal(&mut state);
        let result = state
            .decide(RuleProposalDecisionRequest {
                proposal_id: proposal.proposal_id,
                expected_revision: proposal.revision,
                decision: RuleProposalDecision::Approve,
            })
            .unwrap();

        assert_eq!(result.proposal.status, RuleProposalStatus::Approved);
        let rules = state.active_rules().collect::<Vec<_>>();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].fingerprint, "review-callers");
    }

    #[test]
    fn stale_revision_conflicts() {
        let mut state = ReviewAgentState::default();
        let proposal = pending_proposal(&mut state);

        let error = state
            .decide(RuleProposalDecisionRequest {
                proposal_id: proposal.proposal_id,
                expected_revision: 0,
                decision: RuleProposalDecision::Approve,
            })
            .unwrap_err();
        assert_eq!(
            error,
            DecisionError::RevisionConflict {
                current_revision: 1
            }
        );
    }

    #[test]
    fn repeated_same_decision_is_idempotent() {
        let mut state = ReviewAgentState::default();
        let proposal = pending_proposal(&mut state);
        let request = RuleProposalDecisionRequest {
            proposal_id: proposal.proposal_id,
            expected_revision: proposal.revision,
            decision: RuleProposalDecision::Approve,
        };
        let first = state.decide(request.clone()).unwrap();
        let duplicate = state.decide(request).unwrap();

        assert!(first.changed);
        assert!(!duplicate.changed);
        assert_eq!(state.active_rules().count(), 1);
    }

    #[test]
    fn pending_proposal_keeps_only_threshold_evidence() {
        let mut state = ReviewAgentState::default();
        let proposal = pending_proposal(&mut state);
        let result = state.submit(input("completion-3")).unwrap();

        assert_eq!(
            result.submission.outcome,
            RuleProposalSubmitOutcome::SuppressedPending
        );
        assert!(!result.changed);
        let stored = state
            .proposals()
            .find(|stored| stored.proposal_id == proposal.proposal_id)
            .unwrap();
        assert_eq!(stored.source_event_ids.len(), PROPOSAL_EVIDENCE_THRESHOLD);
        assert_eq!(stored.revision, proposal.revision);
    }

    #[test]
    fn evidence_candidate_count_is_bounded() {
        let mut state = ReviewAgentState::default();
        for index in 0..MAX_EVIDENCE_CANDIDATES {
            let mut candidate = input(&format!("completion-{index}"));
            candidate.fingerprint = format!("candidate-{index}");
            state.submit(candidate).unwrap();
        }

        let mut overflow = input("overflow-event");
        overflow.fingerprint = "overflow-candidate".into();
        assert_eq!(
            state.submit(overflow).unwrap_err(),
            SubmitError::LimitExceeded("rule evidence candidate")
        );
    }

    #[test]
    fn one_source_event_cannot_fill_the_evidence_candidate_store() {
        let mut state = ReviewAgentState::default();
        for index in 0..MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT {
            let mut candidate = input("same-completion");
            candidate.fingerprint = format!("candidate-{index}");
            state.submit(candidate).unwrap();
        }

        let mut duplicate_input = input("same-completion");
        duplicate_input.fingerprint = "candidate-0".into();
        let duplicate = state.submit(duplicate_input).unwrap();
        assert_eq!(
            duplicate.submission.outcome,
            RuleProposalSubmitOutcome::DuplicateEvent
        );

        let mut overflow = input("same-completion");
        overflow.fingerprint = "candidate-overflow".into();
        assert_eq!(
            state.submit(overflow).unwrap_err(),
            SubmitError::LimitExceeded("source event rule observation")
        );
    }

    #[test]
    fn identical_fingerprints_are_independent_between_profiles() {
        let mut state = ReviewAgentState::default();
        let mut profile_a_first = input("profile-a-1");
        profile_a_first.target_profile_id = ReviewBackendProfileId::new("profile-a");
        let mut profile_b_first = input("profile-b-1");
        profile_b_first.target_profile_id = ReviewBackendProfileId::new("profile-b");

        state.submit(profile_a_first.clone()).unwrap();
        state.submit(profile_b_first.clone()).unwrap();
        let profile_a = state
            .submit(RuleProposalSubmitInput {
                source_event_id: "profile-a-2".into(),
                ..profile_a_first
            })
            .unwrap()
            .submission
            .proposal
            .unwrap();
        let profile_b = state
            .submit(RuleProposalSubmitInput {
                source_event_id: "profile-b-2".into(),
                ..profile_b_first
            })
            .unwrap()
            .submission
            .proposal
            .unwrap();

        state
            .decide(RuleProposalDecisionRequest {
                proposal_id: profile_a.proposal_id,
                expected_revision: profile_a.revision,
                decision: RuleProposalDecision::Approve,
            })
            .unwrap();

        assert_eq!(state.proposals().count(), 2);
        assert_eq!(state.active_rules().count(), 1);
        assert_eq!(profile_b.target_profile_id.as_str(), "profile-b");
    }

    #[test]
    fn proposal_count_is_bounded() {
        let mut state = ReviewAgentState::default();
        for index in 0..MAX_RULE_PROPOSALS {
            let mut first = input(&format!("completion-{index}-1"));
            first.fingerprint = format!("proposal-{index}");
            let mut second = first.clone();
            second.source_event_id = format!("completion-{index}-2");
            state.submit(first).unwrap();
            state.submit(second).unwrap();
        }

        let mut first = input("overflow-1");
        first.fingerprint = "overflow-proposal".into();
        let mut second = first.clone();
        second.source_event_id = "overflow-2".into();
        state.submit(first).unwrap();
        assert_eq!(
            state.submit(second).unwrap_err(),
            SubmitError::LimitExceeded("rule proposal")
        );
        assert_eq!(state.proposals().count(), MAX_RULE_PROPOSALS);
    }

    #[test]
    fn validation_rejects_terminal_control_characters() {
        let mut state = ReviewAgentState::default();
        let mut escaped_rule = input("completion-1");
        escaped_rule.rule_text = "safe\u{1b}[31mred".into();
        assert_eq!(
            state.submit(escaped_rule).unwrap_err(),
            SubmitError::InvalidControlCharacter("rule_text")
        );

        let mut escaped_profile = input("completion-1");
        escaped_profile.target_profile_id = ReviewBackendProfileId::new("review\nagent");
        assert_eq!(
            state.submit(escaped_profile).unwrap_err(),
            SubmitError::InvalidControlCharacter("target_profile_id")
        );

        let mut escaped_fingerprint = input("completion-1");
        escaped_fingerprint.fingerprint = "fingerprint\tpart".into();
        assert_eq!(
            state.submit(escaped_fingerprint).unwrap_err(),
            SubmitError::InvalidControlCharacter("fingerprint")
        );

        let escaped_event = input("completion\u{7}");
        assert_eq!(
            state.submit(escaped_event).unwrap_err(),
            SubmitError::InvalidControlCharacter("source_event_id")
        );

        let mut multiline_rule = input("completion-2");
        multiline_rule.rule_text = "first line\n\tsecond line".into();
        assert!(state.submit(multiline_rule).is_ok());
    }
}
