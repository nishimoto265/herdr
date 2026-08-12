use serde::{Deserialize, Serialize};

pub use crate::shitsuji_agent::{
    ActiveRule, RuleProposal, RuleProposalChange, RuleProposalStatus, RuleProposalSubmission,
    ShitsujiBackendProfileId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuleProposalSubmitParams {
    pub rule_text: String,
    pub target_profile_id: ShitsujiBackendProfileId,
    pub fingerprint: String,
    pub source_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct RuleProposalListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RuleProposalStatus>,
}
