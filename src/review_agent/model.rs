use serde::{Deserialize, Serialize};

// Provider-specific transcript adapters are implemented separately from proposal persistence.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptProvider {
    Claude,
    Codex,
}

macro_rules! string_id {
    ($name:ident) => {
        // Some ID wrappers precede the runtime adapters that consume them.
        #[allow(dead_code)]
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        #[allow(dead_code)]
        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(ReviewBackendProfileId);
string_id!(RuleTargetId);
string_id!(RuleProposalId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleProposalStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleProposalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuleProposal {
    pub proposal_id: RuleProposalId,
    pub rule_text: String,
    pub target_profile_id: ReviewBackendProfileId,
    pub fingerprint: String,
    pub source_event_ids: Vec<String>,
    pub status: RuleProposalStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActiveRule {
    pub proposal_id: RuleProposalId,
    pub rule_text: String,
    pub target_profile_id: ReviewBackendProfileId,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuleProposalSubmitInput {
    pub rule_text: String,
    pub target_profile_id: ReviewBackendProfileId,
    pub fingerprint: String,
    pub source_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleProposalDecisionRequest {
    pub proposal_id: RuleProposalId,
    pub expected_revision: u64,
    pub decision: RuleProposalDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleProposalSubmitOutcome {
    EvidenceRecorded,
    ProposalCreated,
    SuppressedPending,
    DuplicateEvent,
    SuppressedApproved,
    SuppressedRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuleProposalSubmission {
    pub outcome: RuleProposalSubmitOutcome,
    pub evidence_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<RuleProposal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleProposalChange {
    Proposed,
    Approved,
    Rejected,
}
