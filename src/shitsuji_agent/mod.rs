pub(crate) mod conversation;
pub(crate) mod delivery;
mod model;
mod transition;

pub use model::{
    ActiveRule, RuleProposal, RuleProposalChange, RuleProposalDecision,
    RuleProposalDecisionRequest, RuleProposalId, RuleProposalStatus, RuleProposalSubmission,
    RuleProposalSubmitInput, RuleProposalSubmitOutcome, ShitsujiBackendProfileId,
};
#[allow(unused_imports)]
pub use model::{RuleTargetId, TranscriptProvider};
pub(crate) use transition::{ShitsujiAgentState, SubmitError, SubmitTransition};

pub(crate) const PROPOSAL_EVIDENCE_THRESHOLD: usize = 2;
pub(crate) const MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT: usize = 32;
/// One profile holding this many approved rules is the signal to start using finer profiles.
/// The intent is "more than 30", encoded as `>= 31` so both overload thresholds compare the same
/// way. Splitting is a separate step; reaching the count only reports the condition.
pub(crate) const ACTIVE_RULES_PER_PROFILE_OVERLOAD_THRESHOLD: usize = 31;
/// Every approved rule of every profile is embedded in the shitsuji prompts, and the prompt
/// builder fails closed past `MAX_ACTIVE_RULES_IN_PROMPT` (`src/app/shitsuji_agent.rs`): a role
/// prompt that cannot be built stops the backend from starting at all. This is three quarters of
/// that limit, because the per-profile threshold above can stay silent while the total crosses it
/// (5 profiles x 13 rules = 65).
pub(crate) const TOTAL_ACTIVE_RULES_OVERLOAD_THRESHOLD: usize = 48;
