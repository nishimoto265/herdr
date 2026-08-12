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
        let expected_profile_id = crate::review_agent::ReviewBackendProfileId::new(
            self.review_agent_config.backend_profile_id.trim(),
        );
        if params.target_profile_id != expected_profile_id {
            return encode_error(
                id,
                "invalid_rule_proposal_target",
                "target profile does not match the configured Review Agent profile",
            );
        }
        if !self
            .review_delivery
            .has_in_flight_source_event(&params.source_event_id)
        {
            return encode_error(
                id,
                "invalid_rule_proposal_source_event",
                "source event is not the Review Agent's current conversation",
            );
        }

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
            self.state.sync_review_panel_proposals(false);
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

    /// Apply a decision originating from Herdr's interactive review panel.
    /// Approve and reject are intentionally not exposed as supported JSON API methods.
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
            self.state.sync_review_panel_proposals(true);
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
        self.state.sync_review_panel_proposals(true);
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
        ErrorResponse, Method, Request, ReviewBackendProfileId, RuleProposalStatus, SuccessResponse,
    };
    use crate::config::Config;

    fn test_app(event_hub: crate::api::EventHub) -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut config = Config::default();
        config.review_agent.backend_profile_id = "review-agent".into();
        App::new(&config, true, None, api_rx, event_hub)
    }

    fn set_in_flight_conversation(app: &mut App) -> String {
        let (delivery, source_event_id) =
            crate::review_agent::delivery::ReviewDeliveryState::with_test_in_flight_conversation();
        app.review_delivery = delivery;
        source_event_id
    }

    fn submit(event: &str, fingerprint: &str) -> RuleProposalSubmitParams {
        RuleProposalSubmitParams {
            rule_text: "Check affected callers.".into(),
            target_profile_id: ReviewBackendProfileId::new("review-agent"),
            fingerprint: fingerprint.into(),
            source_event_id: event.into(),
        }
    }

    #[test]
    fn submit_and_list_use_server_owned_state() {
        let event_hub = crate::api::EventHub::default();
        let mut app = test_app(event_hub.clone());
        let first_event = set_in_flight_conversation(&mut app);
        app.handle_api_request(Request {
            id: "submit-1".into(),
            method: Method::ReviewRuleProposalSubmit(submit(&first_event, "check-callers")),
        });
        let second_event = set_in_flight_conversation(&mut app);
        app.handle_api_request(Request {
            id: "submit-2".into(),
            method: Method::ReviewRuleProposalSubmit(submit(&second_event, "check-callers")),
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
        assert_eq!(app.state.review_panel.proposals.len(), 1);
        assert!(app.state.review_panel.is_expanded());

        let proposal = proposals[0].clone();
        app.decide_review_rule_proposal(crate::review_agent::RuleProposalDecisionRequest {
            proposal_id: proposal.proposal_id,
            expected_revision: proposal.revision,
            decision: crate::review_agent::RuleProposalDecision::Approve,
        })
        .unwrap();
        assert!(app.state.review_panel.proposals.is_empty());
        assert!(!app.state.review_panel.is_expanded());
        assert!(event_hub
            .events_after(0)
            .iter()
            .any(|(_, event)| { event.event == EventKind::ReviewRuleProposalChanged }));
    }

    #[test]
    fn submit_rejects_unknown_past_and_wrong_profile_sources() {
        let mut app = test_app(crate::api::EventHub::default());
        let current_event = set_in_flight_conversation(&mut app);

        let unknown = app.handle_review_rule_proposal_submit(
            "unknown".into(),
            submit("fabricated-event", "check-callers"),
        );
        let unknown: ErrorResponse = serde_json::from_str(&unknown).unwrap();
        assert_eq!(unknown.error.code, "invalid_rule_proposal_source_event");

        let mut wrong_profile = submit(&current_event, "check-callers");
        wrong_profile.target_profile_id = ReviewBackendProfileId::new("another-profile");
        let wrong_profile =
            app.handle_review_rule_proposal_submit("wrong-profile".into(), wrong_profile);
        let wrong_profile: ErrorResponse = serde_json::from_str(&wrong_profile).unwrap();
        assert_eq!(wrong_profile.error.code, "invalid_rule_proposal_target");

        let accepted = app.handle_review_rule_proposal_submit(
            "accepted".into(),
            submit(&current_event, "check-callers"),
        );
        assert!(serde_json::from_str::<SuccessResponse>(&accepted).is_ok());

        let next_event = set_in_flight_conversation(&mut app);
        assert_ne!(next_event, current_event);
        let past = app.handle_review_rule_proposal_submit(
            "past".into(),
            submit(&current_event, "check-callers"),
        );
        let past: ErrorResponse = serde_json::from_str(&past).unwrap();
        assert_eq!(past.error.code, "invalid_rule_proposal_source_event");
        assert_eq!(app.state.review_agent.proposals().count(), 0);
    }

    #[test]
    fn one_in_flight_event_cannot_forge_threshold_or_fill_candidate_store() {
        let mut app = test_app(crate::api::EventHub::default());
        let current_event = set_in_flight_conversation(&mut app);

        let first = app.handle_review_rule_proposal_submit(
            "first".into(),
            submit(&current_event, "check-callers"),
        );
        assert!(serde_json::from_str::<SuccessResponse>(&first).is_ok());
        let fake_second = app.handle_review_rule_proposal_submit(
            "fake-second".into(),
            submit("fabricated-second-event", "check-callers"),
        );
        let fake_second: ErrorResponse = serde_json::from_str(&fake_second).unwrap();
        assert_eq!(fake_second.error.code, "invalid_rule_proposal_source_event");
        assert_eq!(app.state.review_agent.proposals().count(), 0);

        for index in 1..crate::review_agent::MAX_RULE_OBSERVATIONS_PER_SOURCE_EVENT {
            let response = app.handle_review_rule_proposal_submit(
                format!("candidate-{index}"),
                submit(&current_event, &format!("candidate-{index}")),
            );
            assert!(serde_json::from_str::<SuccessResponse>(&response).is_ok());
        }
        let overflow = app.handle_review_rule_proposal_submit(
            "overflow".into(),
            submit(&current_event, "candidate-overflow"),
        );
        let overflow: ErrorResponse = serde_json::from_str(&overflow).unwrap();
        assert_eq!(overflow.error.code, "rule_proposal_limit_exceeded");
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
