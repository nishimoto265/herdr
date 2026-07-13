mod model;
mod transition;

// Transcript association and backend startup consume these identifiers in a later slice; defining
// them now keeps provider, backend profile, and rule target concepts distinct in the core model.
pub use model::{
    ActiveRule, ReviewBackendProfileId, RuleProposal, RuleProposalChange, RuleProposalDecision,
    RuleProposalDecisionRequest, RuleProposalId, RuleProposalStatus, RuleProposalSubmission,
    RuleProposalSubmitInput, RuleProposalSubmitOutcome,
};
#[allow(unused_imports)]
pub use model::{RuleTargetId, TranscriptProvider};
pub(crate) use transition::{ReviewAgentState, SubmitError, SubmitTransition};

pub(crate) const PROPOSAL_EVIDENCE_THRESHOLD: usize = 2;
