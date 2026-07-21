use crate::{
    ActionSpecV1, Capability, ErrorClass, HookPoint, MatchConditionsV1,
    ModelCapabilityRequirementsV1, ProviderModeV1, RerouteActionSpecV1, RetrySameRouteSpecV1,
    RouteScopeKindV1, RuleSetV1, RuleSpecV1, StatusMatcherSpec, API_VERSION_V1,
};

pub const ACCOUNT_FAILOVER_ID: &str = "alex.account-failover";
pub const MODEL_FALLBACKS_ID: &str = "alex.model-fallbacks";
pub const MODEL_EQUIVALENCE_FAILOVER_ID: &str = "alex.model-equivalence-failover";
pub const AUTH_FAILOVER_ID: &str = "alex.auth-failover";
pub const FABLE_TO_SOL_EXAMPLE_ID: &str = "example.fable-overload-to-sol";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFallbackTarget {
    pub model: String,
    pub providers: Vec<String>,
}

/// Shipped same-route account failover policy. The account selector, refresh,
/// cooldown, and credential checks remain proxy responsibilities.
pub fn account_failover_rule(enabled: bool) -> RuleSpecV1 {
    RuleSpecV1 {
        id: ACCOUNT_FAILOVER_ID.into(),
        name: "Account Failover".into(),
        description: Some("Try another eligible account for capacity and server failures.".into()),
        enabled,
        priority: -100,
        hook: HookPoint::AttemptResult,
        capabilities: vec![Capability::RouteOverride],
        when: MatchConditionsV1 {
            status: vec![
                StatusMatcherSpec::Exact(429),
                StatusMatcherSpec::RangeOrClass("500-599".into()),
            ],
            error_classes: vec![
                ErrorClass::Capacity,
                ErrorClass::Server,
                ErrorClass::Network,
            ],
            same_route_accounts_remaining: Some(true),
            ..Default::default()
        },
        expression: None,
        action: ActionSpecV1 {
            retry_same_route: Some(RetrySameRouteSpecV1 {
                exclude_current_account: true,
                reason: "another eligible account is available".into(),
                max_attempts: None,
            }),
            ..Default::default()
        },
    }
}

/// Creates an ordered fallback chain using ordinary public rules. Each rule
/// matches the current attempted model; the coordinator re-enters middleware
/// after each failed fallback.
pub fn ordered_model_fallback_rules(
    enabled: bool,
    original_model: impl Into<String>,
    fallbacks: Vec<ModelFallbackTarget>,
) -> Vec<RuleSpecV1> {
    let original_model = original_model.into();
    let mut current = original_model.clone();
    fallbacks
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let source = std::mem::replace(&mut current, target.model.clone());
            RuleSpecV1 {
                id: format!("{MODEL_FALLBACKS_ID}.{}", index + 1),
                name: format!("Model Fallback {}", index + 1),
                description: Some(format!(
                    "Ordered fallback for {original_model}: {source} to {}.",
                    target.model
                )),
                enabled,
                priority: -200 - index as i32,
                hook: HookPoint::AttemptResult,
                capabilities: vec![Capability::RouteOverride],
                when: MatchConditionsV1 {
                    current_models: vec![source],
                    status: vec![
                        StatusMatcherSpec::Exact(429),
                        StatusMatcherSpec::RangeOrClass("500-599".into()),
                    ],
                    error_classes: vec![
                        ErrorClass::Capacity,
                        ErrorClass::Server,
                        ErrorClass::Network,
                    ],
                    same_route_accounts_remaining: Some(false),
                    ..Default::default()
                },
                expression: None,
                action: ActionSpecV1 {
                    reroute: Some(RerouteActionSpecV1 {
                        model: Some(target.model),
                        equivalent_class: None,
                        providers: target.providers,
                        provider_mode: ProviderModeV1::Only,
                        scope: RouteScopeKindV1::Request,
                        ttl_seconds: None,
                        notice: None,
                        reason: "ordered model fallback".into(),
                        max_attempts: None,
                        required_capabilities: Default::default(),
                    }),
                    ..Default::default()
                },
            }
        })
        .collect()
}

pub fn model_equivalence_failover_rule(
    enabled: bool,
    source_models: Vec<String>,
    equivalence_class: impl Into<String>,
    providers: Vec<String>,
) -> RuleSpecV1 {
    let provider_mode = if providers.is_empty() {
        ProviderModeV1::Any
    } else {
        ProviderModeV1::Prefer
    };
    RuleSpecV1 {
        id: MODEL_EQUIVALENCE_FAILOVER_ID.into(),
        name: "Model Equivalence Failover".into(),
        description: Some("Select another configured model in an equivalence class.".into()),
        enabled,
        priority: -300,
        hook: HookPoint::AttemptResult,
        capabilities: vec![Capability::RouteOverride],
        when: MatchConditionsV1 {
            models: source_models,
            status: vec![
                StatusMatcherSpec::Exact(429),
                StatusMatcherSpec::RangeOrClass("500-599".into()),
            ],
            error_classes: vec![ErrorClass::Capacity, ErrorClass::Server],
            same_route_accounts_remaining: Some(false),
            ..Default::default()
        },
        expression: None,
        action: ActionSpecV1 {
            reroute: Some(RerouteActionSpecV1 {
                model: None,
                equivalent_class: Some(equivalence_class.into()),
                providers,
                provider_mode,
                scope: RouteScopeKindV1::Request,
                ttl_seconds: None,
                notice: None,
                reason: "equivalent model selected after route exhaustion".into(),
                max_attempts: None,
                required_capabilities: Default::default(),
            }),
            ..Default::default()
        },
    }
}

/// Authentication failover stays opt-in and only moves to another eligible
/// account on the same route. Cross-provider auth movement can be expressed by
/// duplicating this template and selecting an exact/equivalent reroute.
pub fn auth_failover_rule(enabled: bool) -> RuleSpecV1 {
    RuleSpecV1 {
        id: AUTH_FAILOVER_ID.into(),
        name: "Authentication Failover".into(),
        description: Some(
            "Try another eligible account after a confirmed authentication failure.".into(),
        ),
        enabled,
        priority: -50,
        hook: HookPoint::AttemptResult,
        capabilities: vec![Capability::RouteOverride],
        when: MatchConditionsV1 {
            status: vec![StatusMatcherSpec::Exact(401), StatusMatcherSpec::Exact(403)],
            error_classes: vec![ErrorClass::Auth],
            same_route_accounts_remaining: Some(true),
            ..Default::default()
        },
        expression: None,
        action: ActionSpecV1 {
            retry_same_route: Some(RetrySameRouteSpecV1 {
                exclude_current_account: true,
                reason: "confirmed authentication failure on current account".into(),
                max_attempts: None,
            }),
            ..Default::default()
        },
    }
}

/// Concrete example used by documentation, dry-run tests, and the future Tom's
/// Middleware Wizard golden test.
pub fn fable_to_sol_rule() -> RuleSpecV1 {
    RuleSpecV1 {
        id: FABLE_TO_SOL_EXAMPLE_ID.into(),
        name: "Move overloaded Fable chats to Sol".into(),
        description: Some("Reroute selected failed Anthropic Fable requests to OpenAI Sol.".into()),
        enabled: true,
        priority: 100,
        hook: HookPoint::AttemptResult,
        capabilities: vec![
            Capability::AttemptReadErrorBody,
            Capability::RouteOverride,
            Capability::SessionPin,
            Capability::ResponsePrependText,
        ],
        when: MatchConditionsV1 {
            harness_names: vec!["claude".into(), "codex".into(), "pi".into()],
            models: vec!["claude-fable-5".into(), "fable-*".into()],
            providers: vec!["anthropic".into()],
            status: vec![
                StatusMatcherSpec::Exact(429),
                StatusMatcherSpec::RangeOrClass("500-599".into()),
            ],
            error_classes: vec![ErrorClass::Capacity, ErrorClass::Server],
            body_contains_any: vec![
                "model is currently overloaded".into(),
                "subscription is unavailable".into(),
            ],
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
                ttl_seconds: Some(86_400),
                notice: Some("We moved this chat from Fable 5 to GPT 5.6 Sol.".into()),
                reason: "Fable failed with a selected overload or availability error".into(),
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
        rules: vec![
            account_failover_rule(true),
            model_equivalence_failover_rule(false, Vec::new(), "default", Vec::new()),
            auth_failover_rule(false),
        ],
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

    fn fable_context(with_body: bool) -> AttemptResultContext {
        let model = ModelRef {
            provider: "anthropic".into(),
            id: "claude-fable-5".into(),
            aliases: vec!["fable-5".into()],
            equivalence_classes: vec!["premium".into()],
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
            harness: HarnessView {
                name: Some("claude".into()),
                version: Some("1.2.3".into()),
            },
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
                status: 529,
                headers: SafeHeaders::default(),
                body: if with_body {
                    let text = r#"{"type":"error","error":{"type":"overloaded_error","message":"model is currently overloaded"}}"#;
                    BodyView {
                        content_type: Some("application/json".into()),
                        size_bytes: Some(text.len() as u64),
                        text: Some(text.into()),
                        json: serde_json::from_str(text).ok(),
                        truncated: false,
                        inspected_bytes: text.len(),
                    }
                } else {
                    BodyView {
                        content_type: Some("application/json".into()),
                        size_bytes: None,
                        ..Default::default()
                    }
                },
                error: Some(ErrorInfo {
                    class: ErrorClass::Capacity,
                    kind: Some("overloaded_error".into()),
                    code: None,
                    message: Some("model is currently overloaded".into()),
                }),
                timing: Default::default(),
            },
        }
    }

    #[test]
    fn every_builtin_uses_and_compiles_through_public_rule_schema() {
        let mut rules = default_builtin_rule_set().rules;
        rules.extend(ordered_model_fallback_rules(
            true,
            "model-a",
            vec![ModelFallbackTarget {
                model: "model-b".into(),
                providers: vec!["openai".into()],
            }],
        ));
        rules.push(fable_to_sol_rule());
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules,
        })
        .unwrap();
        assert!(engine.rule_ids().any(|id| id == ACCOUNT_FAILOVER_ID));
        assert!(engine.rule_ids().any(|id| id == FABLE_TO_SOL_EXAMPLE_ID));
    }

    #[test]
    fn fable_error_requires_body_then_reroutes_to_sol_for_session() {
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![fable_to_sol_rule()],
        })
        .unwrap();
        let head = fable_context(false);
        let plan = engine.inspection_plan(&head);
        assert!(plan.needs_body);
        assert_eq!(plan.candidate_rule_ids, vec![FABLE_TO_SOL_EXAMPLE_ID]);

        let result = engine.evaluate_attempt(&fable_context(true));
        assert_eq!(result.records[0].rule_id, FABLE_TO_SOL_EXAMPLE_ID);
        assert_eq!(
            result.decision,
            crate::AttemptDecision::Reroute {
                target: RouteTarget::Exact {
                    model: "gpt-5.6-sol".into(),
                    providers: ProviderConstraint::Only(vec!["openai".into()]),
                },
                scope: RouteScope::Session {
                    ttl_seconds: 86_400
                },
                notice: Some(crate::ResponseNotice {
                    text: "We moved this chat from Fable 5 to GPT 5.6 Sol.".into()
                }),
                reason: "Fable failed with a selected overload or availability error".into(),
            }
        );
    }

    #[test]
    fn fable_server_error_without_verified_signal_does_not_match() {
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![fable_to_sol_rule()],
        })
        .unwrap();
        let mut context = fable_context(true);
        let text = r#"{"type":"error","error":{"type":"api_error","message":"A request validation subsystem failed"}}"#;
        context.outcome.status = 500;
        context.outcome.body = BodyView {
            content_type: Some("application/json".into()),
            size_bytes: Some(text.len() as u64),
            text: Some(text.into()),
            json: serde_json::from_str(text).ok(),
            truncated: false,
            inspected_bytes: text.len(),
        };
        context.outcome.error = Some(ErrorInfo {
            class: ErrorClass::Server,
            kind: Some("api_error".into()),
            code: None,
            message: Some("A request validation subsystem failed".into()),
        });

        let result = engine.evaluate_attempt(&context);
        assert_eq!(result.decision, crate::AttemptDecision::Continue);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].state, crate::MatchState::NotMatched);
    }

    #[test]
    fn no_substitute_suppresses_fable_reroute() {
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![fable_to_sol_rule()],
        })
        .unwrap();
        let result = engine.evaluate_attempt_with(
            &fable_context(true),
            EvaluationControl {
                no_substitute: true,
            },
        );
        assert_eq!(result.decision, crate::AttemptDecision::Continue);
        assert!(result.records[0].suppressed);
    }
}
