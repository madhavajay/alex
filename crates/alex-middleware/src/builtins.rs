use crate::{
    ActionSpecV1, Capability, HookPoint, MatchConditionsV1, ModelCapabilityRequirementsV1,
    ProviderModeV1, RerouteActionSpecV1, RouteScopeKindV1, RuleSetV1, RuleSpecV1, API_VERSION_V1,
};

/// The one middleware rule shipped by Alex. Users can inspect and edit it with
/// the same public rule schema used by the Middleware Wizard.
pub const FABLE_TO_SOL_ID: &str = "alex.fable-5-to-gpt-5.6-sol";
pub const FABLE_REFUSAL_KIND: &str = "upstream_refusal";
pub const FABLE_SESSION_TTL_SECONDS: u64 = 24 * 60 * 60;

/// If Anthropic Fable emits its structured SSE refusal signal, retry with
/// high-effort GPT-5.6 Sol and keep that route for the stable session.
pub fn fable_to_sol_rule() -> RuleSpecV1 {
    RuleSpecV1 {
        id: FABLE_TO_SOL_ID.into(),
        name: "Fable 5 → GPT-5.6 Sol".into(),
        description: Some(
            "When Anthropic Fable 5 refuses a request, switch the session to high-effort GPT-5.6 Sol."
                .into(),
        ),
        enabled: true,
        priority: 100,
        hook: HookPoint::AttemptResult,
        capabilities: vec![Capability::RouteOverride, Capability::SessionPin],
        when: MatchConditionsV1 {
            models: vec!["claude-fable-5".into()],
            providers: vec!["anthropic".into()],
            error_kinds: vec![FABLE_REFUSAL_KIND.into()],
            stable_session: Some(true),
            ..Default::default()
        },
        expression: None,
        action: ActionSpecV1 {
            reroute: Some(RerouteActionSpecV1 {
                model: Some("gpt-5.6-sol".into()),
                equivalent_class: None,
                providers: vec!["openai".into()],
                provider_mode: ProviderModeV1::Only,
                scope: RouteScopeKindV1::Session,
                ttl_seconds: Some(FABLE_SESSION_TTL_SECONDS),
                notice: None,
                effort: Some("high".into()),
                reason: "Fable 5 refused the request; switching this session to GPT-5.6 Sol".into(),
                max_attempts: Some(3),
                required_capabilities: ModelCapabilityRequirementsV1 {
                    portable_history: true,
                    ..Default::default()
                },
            }),
            ..Default::default()
        },
    }
}

pub fn default_builtin_rule_set() -> RuleSetV1 {
    RuleSetV1 {
        api_version: API_VERSION_V1,
        rules: vec![fable_to_sol_rule()],
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AttemptOutcome, AttemptResultContext, BodyView, ClientFormat, ClientRequestView,
        CompiledRuleSetV1, ErrorClass, ErrorInfo, EvaluationControl, HarnessView, JsonBodyView,
        ModelCapabilities, ModelRef, ProviderConstraint, ProviderView, RouteScope, RouteTarget,
        RouteView, SafeHeaders, SessionIdSource, SessionView,
    };

    use serde_json::json;

    use super::*;

    fn fable_refusal_context(category: &str) -> AttemptResultContext {
        let model = ModelRef {
            provider: "anthropic".into(),
            id: "claude-fable-5".into(),
            aliases: vec!["fable-5".into()],
            equivalence_classes: Vec::new(),
            capabilities: ModelCapabilities {
                tools: true,
                portable_history: true,
                ..Default::default()
            },
        };
        let body = format!(
            "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"refusal\",\"stop_details\":{{\"type\":\"refusal\",\"category\":\"{category}\"}}}}}}\n\n"
        );
        AttemptResultContext {
            request: ClientRequestView {
                trace_id: "trace-1".into(),
                method: "POST".into(),
                path: "/v1/messages".into(),
                client_format: ClientFormat::AnthropicMessages,
                original_model: "claude-fable-5".into(),
                current_model: "claude-fable-5".into(),
                streaming: true,
                headers: SafeHeaders::default(),
                body: JsonBodyView::default(),
            },
            harness: HarnessView::default(),
            session: SessionView {
                id: Some("session-1".into()),
                run_id: None,
                source: SessionIdSource::NativeHeader,
                active_route_lease: None,
            },
            route: RouteView {
                requested: model.clone(),
                selected: model,
                provider: ProviderView {
                    id: "anthropic".into(),
                    enabled: true,
                    paused: false,
                    healthy: true,
                    supported_formats: vec!["anthropic".into()],
                },
                upstream_format: "anthropic".into(),
                attempt_number: 1,
                same_route_accounts_remaining: false,
            },
            outcome: AttemptOutcome {
                status: 200,
                headers: SafeHeaders::default(),
                body: BodyView {
                    content_type: Some("text/event-stream".into()),
                    size_bytes: Some(body.len() as u64),
                    text: Some(body),
                    json: None,
                    truncated: false,
                    inspected_bytes: 0,
                },
                error: Some(ErrorInfo {
                    class: ErrorClass::Other,
                    kind: Some(FABLE_REFUSAL_KIND.into()),
                    code: Some(category.into()),
                    message: None,
                }),
                timing: Default::default(),
            },
        }
    }

    #[test]
    fn default_rule_set_contains_only_fable_to_sol() {
        let rules = default_builtin_rule_set().rules;
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, FABLE_TO_SOL_ID);
        CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: API_VERSION_V1,
            rules,
        })
        .unwrap();
    }

    #[test]
    fn any_fable_refusal_reroutes_to_sol_for_the_session() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        for category in ["bio", "cyber"] {
            let result = engine.evaluate_attempt(&fable_refusal_context(category));
            assert_eq!(result.records[0].rule_id, FABLE_TO_SOL_ID);
            assert_eq!(
                result.decision,
                crate::AttemptDecision::Reroute {
                    target: RouteTarget::Exact {
                        model: "gpt-5.6-sol".into(),
                        providers: ProviderConstraint::Only(vec!["openai".into()]),
                    },
                    scope: RouteScope::Session {
                        ttl_seconds: FABLE_SESSION_TTL_SECONDS,
                    },
                    notice: None,
                    reason: "Fable 5 refused the request; switching this session to GPT-5.6 Sol"
                        .into(),
                }
            );
        }
    }

    #[test]
    fn fable_overload_is_not_the_refusal_guardrail() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        let mut context = fable_refusal_context("bio");
        context.outcome.status = 529;
        context.outcome.error = Some(ErrorInfo {
            class: ErrorClass::Capacity,
            kind: Some("overloaded_error".into()),
            code: Some("529".into()),
            message: Some("Overloaded".into()),
        });
        assert_eq!(
            engine.evaluate_attempt(&context).decision,
            crate::AttemptDecision::Continue
        );
    }

    #[test]
    fn optional_effort_condition_matches_the_incoming_request() {
        let mut rule = fable_to_sol_rule();
        rule.when.efforts = vec!["high".into()];
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: API_VERSION_V1,
            rules: vec![rule],
        })
        .unwrap();
        let mut context = fable_refusal_context("bio");
        context.request.body.json = Some(json!({"output_config": {"effort": "high"}}));
        assert!(matches!(
            engine.evaluate_attempt(&context).decision,
            crate::AttemptDecision::Reroute { .. }
        ));

        context.request.body.json = Some(json!({"output_config": {"effort": "low"}}));
        assert_eq!(
            engine.evaluate_attempt(&context).decision,
            crate::AttemptDecision::Continue
        );
    }

    #[test]
    fn no_substitute_suppresses_fable_reroute() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        let result = engine.evaluate_attempt_with(
            &fable_refusal_context("bio"),
            EvaluationControl {
                no_substitute: true,
            },
        );
        assert_eq!(result.decision, crate::AttemptDecision::Continue);
        assert!(result.records[0].suppressed);
    }
}
