use crate::{
    ActionSpecV1, Capability, ErrorClass, HookPoint, MatchConditionsV1, ProviderModeV1,
    RerouteActionSpecV1, RouteScopeKindV1, RuleSetV1, RuleSpecV1, StatusMatcherSpec,
    API_VERSION_V1,
};

/// The one middleware rule shipped by Alex. Users can inspect and edit it with
/// the same public rule schema used by the Middleware Wizard.
pub const FABLE_TO_SOL_ID: &str = "alex.fable-5-to-gpt-5.6-sol";

/// If Anthropic cannot serve Fable 5 because of capacity or a provider error,
/// retry this request with GPT-5.6 Sol through OpenAI.
pub fn fable_to_sol_rule() -> RuleSpecV1 {
    RuleSpecV1 {
        id: FABLE_TO_SOL_ID.into(),
        name: "Fable 5 → GPT-5.6 Sol".into(),
        description: Some(
            "If Fable 5 fails with a capacity or provider error, retry with GPT-5.6 Sol.".into(),
        ),
        enabled: true,
        priority: 100,
        hook: HookPoint::AttemptResult,
        capabilities: vec![Capability::RouteOverride],
        when: MatchConditionsV1 {
            models: vec!["claude-fable-5".into()],
            providers: vec!["anthropic".into()],
            status: vec![
                StatusMatcherSpec::Exact(429),
                StatusMatcherSpec::RangeOrClass("500-599".into()),
            ],
            error_classes: vec![ErrorClass::Capacity, ErrorClass::Server],
            ..Default::default()
        },
        expression: None,
        action: ActionSpecV1 {
            reroute: Some(RerouteActionSpecV1 {
                model: Some("gpt-5.6-sol".into()),
                equivalent_class: None,
                providers: vec!["openai".into()],
                provider_mode: ProviderModeV1::Only,
                scope: RouteScopeKindV1::Request,
                ttl_seconds: None,
                notice: None,
                reason: "Fable 5 failed; retrying with GPT-5.6 Sol".into(),
                max_attempts: Some(3),
                required_capabilities: Default::default(),
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
        CompiledRuleSetV1, ErrorInfo, EvaluationControl, HarnessView, JsonBodyView,
        ModelCapabilities, ModelRef, ProviderConstraint, ProviderView, RouteScope, RouteTarget,
        RouteView, SafeHeaders, SessionIdSource, SessionView,
    };

    use super::*;

    fn fable_context(error_class: ErrorClass, status: u16) -> AttemptResultContext {
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
                status,
                headers: SafeHeaders::default(),
                body: BodyView::default(),
                error: Some(ErrorInfo {
                    class: error_class,
                    kind: None,
                    code: Some(status.to_string()),
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
    fn fable_capacity_failure_reroutes_to_sol_for_this_request() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        let context = fable_context(ErrorClass::Capacity, 529);
        let plan = engine.inspection_plan(&context);
        assert!(!plan.needs_body);

        let result = engine.evaluate_attempt(&context);
        assert_eq!(result.records[0].rule_id, FABLE_TO_SOL_ID);
        assert_eq!(
            result.decision,
            crate::AttemptDecision::Reroute {
                target: RouteTarget::Exact {
                    model: "gpt-5.6-sol".into(),
                    providers: ProviderConstraint::Only(vec!["openai".into()]),
                },
                scope: RouteScope::Request,
                notice: None,
                reason: "Fable 5 failed; retrying with GPT-5.6 Sol".into(),
            }
        );
    }

    #[test]
    fn fable_bad_request_does_not_reroute() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        let result = engine.evaluate_attempt(&fable_context(ErrorClass::BadRequest, 400));
        assert_eq!(result.decision, crate::AttemptDecision::Continue);
    }

    #[test]
    fn no_substitute_suppresses_fable_reroute() {
        let engine = CompiledRuleSetV1::compile(default_builtin_rule_set()).unwrap();
        let result = engine.evaluate_attempt_with(
            &fable_context(ErrorClass::Server, 500),
            EvaluationControl {
                no_substitute: true,
            },
        );
        assert_eq!(result.decision, crate::AttemptDecision::Continue);
        assert!(result.records[0].suppressed);
    }
}
