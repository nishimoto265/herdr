pub(crate) mod conversation;
pub(crate) mod delivery;
mod model;
mod transition;

pub use model::{
    ActiveRule, ReviewBackendProfileId, RuleProposal, RuleProposalChange, RuleProposalDecision,
    RuleProposalDecisionRequest, RuleProposalId, RuleProposalStatus, RuleProposalSubmission,
    RuleProposalSubmitInput, RuleProposalSubmitOutcome,
};
#[allow(unused_imports)]
pub use model::{RuleTargetId, TranscriptProvider};
pub(crate) use transition::{ReviewAgentState, SubmitError, SubmitTransition};

pub(crate) const PROPOSAL_EVIDENCE_THRESHOLD: usize = 2;
pub(crate) const MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT: usize = 32;
