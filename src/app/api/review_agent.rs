use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, RuleProposalListParams,
    RuleProposalSubmitParams,
};
use crate::app::App;
use crate::review_agent::{
    RuleProposal, RuleProposalDecisionRequest, RuleProposalSubmitInput, SubmitError,
    SubmitTransition,
};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_review_rule_proposal_submit(
        &mut self,
        id: String,
        params: RuleProposalSubmitParams,
    ) -> String {
        let previous = self.state.review_agent.clone();
        let transition = match self.state.review_agent.submit(RuleProposalSubmitInput {
            rule_text: params.rule_text,
            target_profile_id: params.target_profile_id,
            fingerprint: params.fingerprint,
            source_event_id: params.source_event_id,
        }) {
            Ok(transition) => transition,
            Err(err @ SubmitError::LimitExceeded(_)) => {
                return encode_error(id, "rule_proposal_limit_exceeded", err.to_string());
            }
            Err(err) => return encode_error(id, "invalid_rule_proposal", err.to_string()),
        };
        if transition.changed {
            if let Err(err) = self.save_review_agent_state() {
                self.state.review_agent = previous;
                return encode_error(id, "review_agent_store_failed", err.to_string());
            }
            self.emit_review_proposal_transition(&transition);
        }
        encode_success(
            id,
            ResponseResult::RuleProposalSubmitted {
                submission: transition.submission,
            },
        )
    }

    pub(super) fn handle_review_rule_proposal_list(
        &mut self,
        id: String,
        params: RuleProposalListParams,
    ) -> String {
        let proposals = self
            .state
            .review_agent
            .proposals()
            .filter(|proposal| params.status.is_none_or(|status| proposal.status == status))
            .cloned()
            .collect();
        let active_rules = self.state.review_agent.active_rules().cloned().collect();
        encode_success(
            id,
            ResponseResult::RuleProposalList {
                proposals,
                active_rules,
            },
        )
    }

    /// Apply a human decision originating from Herdr's trusted interactive client path.
    /// This is intentionally not a public JSON API method because pane processes inherit
    /// access to that socket and must not be able to approve their own proposals.
    pub(crate) fn decide_review_rule_proposal(
        &mut self,
        request: RuleProposalDecisionRequest,
    ) -> Result<RuleProposal, String> {
        let previous = self.state.review_agent.clone();
        let transition = self
            .state
            .review_agent
            .decide(request)
            .map_err(|err| err.to_string())?;
        if !transition.changed {
            return Ok(transition.proposal);
        }
        if let Err(err) = self.save_review_agent_state() {
            self.state.review_agent = previous;
            return Err(format!("failed to save review agent state: {err}"));
        }
        if let Some(change) = transition.proposal_change {
            self.emit_event(EventEnvelope {
                event: EventKind::ReviewRuleProposalChanged,
                data: EventData::ReviewRuleProposalChanged {
                    proposal: transition.proposal.clone(),
                    change,
                },
            });
        }
        Ok(transition.proposal)
    }

    fn emit_review_proposal_transition(&mut self, transition: &SubmitTransition) {
        let (Some(proposal), Some(change)) = (
            transition.submission.proposal.clone(),
            transition.proposal_change,
        ) else {
            return;
        };
        self.emit_event(EventEnvelope {
            event: EventKind::ReviewRuleProposalChanged,
            data: EventData::ReviewRuleProposalChanged { proposal, change },
        });
    }

    fn save_review_agent_state(&self) -> std::io::Result<()> {
        if self.no_session {
            return Ok(());
        }
        crate::persist::review_agent::save(&self.state.review_agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{
        Method, Request, ReviewBackendProfileId, RuleProposalStatus, SuccessResponse,
    };
    use crate::config::Config;

    fn test_app(event_hub: crate::api::EventHub) -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(&Config::default(), true, None, api_rx, event_hub)
    }

    fn submit(event: &str) -> RuleProposalSubmitParams {
        RuleProposalSubmitParams {
            rule_text: "Check affected callers.".into(),
            target_profile_id: ReviewBackendProfileId::new("review-agent"),
            fingerprint: "check-callers".into(),
            source_event_id: event.into(),
        }
    }

    #[test]
    fn submit_and_list_use_server_owned_state() {
        let event_hub = crate::api::EventHub::default();
        let mut app = test_app(event_hub.clone());
        app.handle_api_request(Request {
            id: "submit-1".into(),
            method: Method::ReviewRuleProposalSubmit(submit("event-1")),
        });
        app.handle_api_request(Request {
            id: "submit-2".into(),
            method: Method::ReviewRuleProposalSubmit(submit("event-2")),
        });

        let response = app.handle_api_request(Request {
            id: "list".into(),
            method: Method::ReviewRuleProposalList(RuleProposalListParams {
                status: Some(RuleProposalStatus::Pending),
            }),
        });
        let response: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::RuleProposalList { proposals, .. } = response.result else {
            panic!("expected proposal list response");
        };
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].source_event_ids.len(), 2);
        assert!(event_hub
            .events_after(0)
            .iter()
            .any(|(_, event)| { event.event == EventKind::ReviewRuleProposalChanged }));
    }

    #[test]
    fn public_method_enum_has_no_decision_variant() {
        let request = serde_json::json!({
            "id": "decision",
            "method": "review.rule_proposal.decide",
            "params": {
                "proposal_id": "rule-proposal-1",
                "expected_revision": 1,
                "decision": "approve"
            }
        });
        assert!(serde_json::from_value::<Request>(request).is_err());
    }
}
