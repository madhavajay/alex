use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use aho_corasick::AhoCorasick;
use regex::Regex;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    compile_glob_regex, parse_status, starts_like_version_requirement, validate_rule_set,
    AttemptDecision, AttemptResultContext, ErrorClass, HeaderPatch, HookPoint, MatchConditionsV1,
    MatchExpressionV1, ProviderConstraint, ProviderModeV1, RequestReceivedContext, ResponseNotice,
    ResponsePatch, ResponseReadyContext, RouteScope, RouteScopeKindV1, RouteTarget, RuleSetV1,
    RuleSpecV1, ValidationCatalog, ValidationError, ValidationOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchState {
    Matched,
    NotMatched,
    NeedsBody,
}

impl MatchState {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::NotMatched, _) | (_, Self::NotMatched) => Self::NotMatched,
            (Self::NeedsBody, _) | (_, Self::NeedsBody) => Self::NeedsBody,
            _ => Self::Matched,
        }
    }

    fn invert(self) -> Self {
        match self {
            Self::Matched => Self::NotMatched,
            Self::NotMatched => Self::Matched,
            Self::NeedsBody => Self::NeedsBody,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvaluationControl {
    /// Suppresses account/model retry and reroute decisions. The proxy should set
    /// this from its authenticated `x-alex-no-substitute` handling.
    pub no_substitute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleEvaluationRecord {
    pub rule_id: String,
    pub state: MatchState,
    pub action: Option<String>,
    #[serde(default)]
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleConditionDebugRecord {
    pub group: String,
    pub state: MatchState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDebugEvaluation {
    pub rule_id: String,
    pub state: MatchState,
    #[serde(default)]
    pub conditions: Vec<RuleConditionDebugRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub decision: AttemptDecision,
    #[serde(default)]
    pub request_header_patches: Vec<HeaderPatch>,
    #[serde(default)]
    pub response_patches: Vec<ResponsePatch>,
    #[serde(default)]
    pub records: Vec<RuleEvaluationRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PatchEvaluationResult {
    #[serde(default)]
    pub request_header_patches: Vec<HeaderPatch>,
    #[serde(default)]
    pub response_patches: Vec<ResponsePatch>,
    #[serde(default)]
    pub records: Vec<RuleEvaluationRecord>,
}

impl Default for EvaluationResult {
    fn default() -> Self {
        Self {
            decision: AttemptDecision::Continue,
            request_header_patches: Vec::new(),
            response_patches: Vec::new(),
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyInspectionPlan {
    pub needs_body: bool,
    pub needs_json: bool,
    pub candidate_rule_ids: Vec<String>,
}

#[derive(Debug, Error)]
#[error("middleware rule set has {errors_len} validation error(s)")]
pub struct CompileError {
    pub errors: Vec<ValidationError>,
    errors_len: usize,
}

impl CompileError {
    fn new(errors: Vec<ValidationError>) -> Self {
        let errors_len = errors.len();
        Self { errors, errors_len }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledRuleSetV1 {
    rules: Vec<CompiledRule>,
    index: RuleIndex,
}

impl CompiledRuleSetV1 {
    pub fn compile(rule_set: RuleSetV1) -> Result<Self, CompileError> {
        Self::compile_with(rule_set, &ValidationOptions::default(), None)
    }

    pub fn compile_with(
        rule_set: RuleSetV1,
        options: &ValidationOptions,
        catalog: Option<&ValidationCatalog>,
    ) -> Result<Self, CompileError> {
        let errors = validate_rule_set(&rule_set, options, catalog);
        if !errors.is_empty() {
            return Err(CompileError::new(errors));
        }
        let mut rules: Vec<_> = rule_set
            .rules
            .into_iter()
            .filter(|rule| rule.enabled)
            .map(CompiledRule::compile)
            .collect();
        rules.sort_by(|left, right| {
            right
                .spec
                .priority
                .cmp(&left.spec.priority)
                .then_with(|| left.spec.id.cmp(&right.spec.id))
        });
        let index = RuleIndex::build(&rules);
        Ok(Self { rules, index })
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().map(|rule| rule.spec.id.as_str())
    }

    pub fn active_rules(&self) -> impl Iterator<Item = &RuleSpecV1> {
        self.rules.iter().map(|rule| &rule.spec)
    }

    pub fn has_attempt_body_rules(&self) -> bool {
        self.rules.iter().any(|rule| {
            rule.spec.hook == HookPoint::AttemptResult
                && (rule.spec.when.needs_body()
                    || rule
                        .spec
                        .expression
                        .as_ref()
                        .is_some_and(MatchExpressionV1::needs_body))
        })
    }

    pub fn has_debug_attempt_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.spec.debug && rule.spec.hook == HookPoint::AttemptResult)
    }

    /// Returns the body work required solely for opt-in diagnostics. Unlike
    /// the indexed execution path this deliberately considers every debug
    /// rule so a rule excluded by a cheap condition still gets a no-match log.
    pub fn debug_inspection_plan(&self, context: &AttemptResultContext) -> BodyInspectionPlan {
        let mut plan = BodyInspectionPlan::default();
        for rule in self
            .rules
            .iter()
            .filter(|rule| rule.spec.debug && rule.spec.hook == HookPoint::AttemptResult)
        {
            if rule.matches(context) == MatchState::NeedsBody {
                plan.needs_body = true;
                plan.needs_json |= rule.needs_json_body;
                plan.candidate_rule_ids.push(rule.spec.id.clone());
            }
        }
        plan
    }

    pub fn evaluate_debug_attempt(
        &self,
        context: &AttemptResultContext,
    ) -> Vec<RuleDebugEvaluation> {
        self.rules
            .iter()
            .filter(|rule| rule.spec.debug && rule.spec.hook == HookPoint::AttemptResult)
            .map(|rule| rule.debug_evaluation(context))
            .collect()
    }

    /// Returns a conservative plan from response-head and request metadata. A
    /// body-free rule can still produce a decision, but is not listed here.
    pub fn inspection_plan(&self, context: &AttemptResultContext) -> BodyInspectionPlan {
        let mut plan = BodyInspectionPlan::default();
        for index in self.index.attempt_candidates(context) {
            let rule = &self.rules[index];
            if rule.matches(context) == MatchState::NeedsBody {
                plan.needs_body = true;
                plan.needs_json |= rule.needs_json_body;
                plan.candidate_rule_ids.push(rule.spec.id.clone());
            }
        }
        plan
    }

    pub fn evaluate_attempt(&self, context: &AttemptResultContext) -> EvaluationResult {
        self.evaluate_attempt_with(context, EvaluationControl::default())
    }

    pub fn evaluate_attempt_with(
        &self,
        context: &AttemptResultContext,
        control: EvaluationControl,
    ) -> EvaluationResult {
        let mut result = EvaluationResult::default();
        for index in self.index.attempt_candidates(context) {
            let rule = &self.rules[index];
            let state = rule.matches(context);
            if state != MatchState::Matched {
                result.records.push(RuleEvaluationRecord {
                    rule_id: rule.spec.id.clone(),
                    state,
                    action: None,
                    suppressed: false,
                });
                continue;
            }

            let decision = rule.decision();
            let routing_decision = matches!(
                decision,
                AttemptDecision::RetrySameRoute { .. } | AttemptDecision::Reroute { .. }
            );
            if control.no_substitute && routing_decision {
                result.records.push(RuleEvaluationRecord {
                    rule_id: rule.spec.id.clone(),
                    state,
                    action: Some(decision_name(&decision).to_owned()),
                    suppressed: true,
                });
                continue;
            }

            result
                .request_header_patches
                .extend(rule.spec.action.request_headers.clone());
            result
                .response_patches
                .extend(rule.spec.action.response_patches.clone());
            result.records.push(RuleEvaluationRecord {
                rule_id: rule.spec.id.clone(),
                state,
                action: Some(decision_name(&decision).to_owned()),
                suppressed: false,
            });
            if decision.is_terminal() {
                result.decision = decision;
                break;
            }
        }
        result
    }

    /// Evaluates request-stage middleware. Patches from every matching rule
    /// accumulate in deterministic priority/ID order.
    pub fn evaluate_request_received(
        &self,
        context: &RequestReceivedContext,
    ) -> PatchEvaluationResult {
        let mut result = PatchEvaluationResult::default();
        for index in self
            .index
            .by_hook
            .get(&HookPoint::RequestReceived)
            .into_iter()
            .flatten()
            .copied()
        {
            let rule = &self.rules[index];
            let state = rule.matches_request(context);
            if state == MatchState::Matched {
                result
                    .request_header_patches
                    .extend(rule.spec.action.request_headers.clone());
            }
            result.records.push(RuleEvaluationRecord {
                rule_id: rule.spec.id.clone(),
                state,
                action: (state == MatchState::Matched).then_some(
                    if rule.spec.action.request_headers.is_empty() {
                        "continue".to_owned()
                    } else {
                        "request_patch".to_owned()
                    },
                ),
                suppressed: false,
            });
        }
        result
    }

    /// Evaluates the final buffered response stage. Routing decisions are
    /// structurally impossible here because the loader rejects them.
    pub fn evaluate_response_ready(&self, context: &ResponseReadyContext) -> PatchEvaluationResult {
        let mut result = PatchEvaluationResult::default();
        for index in self
            .index
            .by_hook
            .get(&HookPoint::ResponseReady)
            .into_iter()
            .flatten()
            .copied()
        {
            let rule = &self.rules[index];
            let state = rule.matches_response(context);
            if state == MatchState::Matched {
                result
                    .response_patches
                    .extend(rule.spec.action.response_patches.clone());
            }
            result.records.push(RuleEvaluationRecord {
                rule_id: rule.spec.id.clone(),
                state,
                action: (state == MatchState::Matched).then_some(
                    if rule.spec.action.response_patches.is_empty() {
                        "continue".to_owned()
                    } else {
                        "response_patch".to_owned()
                    },
                ),
                suppressed: false,
            });
        }
        result
    }
}

fn decision_name(decision: &AttemptDecision) -> &'static str {
    match decision {
        AttemptDecision::Continue => "continue",
        AttemptDecision::ReturnOriginal { .. } => "return_original",
        AttemptDecision::RetrySameRoute { .. } => "retry_same_route",
        AttemptDecision::Reroute { .. } => "reroute",
    }
}

#[derive(Debug, Clone)]
struct CompiledRule {
    spec: RuleSpecV1,
    conditions: CompiledConditions,
    expression: Option<CompiledExpression>,
    debug_conditions: Vec<(String, CompiledConditions)>,
    needs_json_body: bool,
}

impl CompiledRule {
    fn compile(spec: RuleSpecV1) -> Self {
        let conditions = CompiledConditions::compile(&spec.when);
        let expression = spec.expression.as_ref().map(CompiledExpression::compile);
        let debug_conditions = spec
            .debug
            .then(|| debug_condition_groups(&spec.when))
            .unwrap_or_default();
        let needs_json_body = spec.when.needs_json_body()
            || spec
                .expression
                .as_ref()
                .is_some_and(MatchExpressionV1::needs_json_body);
        Self {
            spec,
            conditions,
            expression,
            debug_conditions,
            needs_json_body,
        }
    }

    fn matches(&self, context: &AttemptResultContext) -> MatchState {
        let base = self.conditions.matches_attempt(context);
        match &self.expression {
            Some(expression) => base.and(expression.matches_attempt(context)),
            None => base,
        }
    }

    fn debug_evaluation(&self, context: &AttemptResultContext) -> RuleDebugEvaluation {
        let mut conditions = self
            .debug_conditions
            .iter()
            .map(|(group, condition)| RuleConditionDebugRecord {
                group: group.clone(),
                state: condition.matches_attempt(context),
            })
            .collect::<Vec<_>>();
        if let Some(expression) = &self.expression {
            conditions.push(RuleConditionDebugRecord {
                group: "expression".into(),
                state: expression.matches_attempt(context),
            });
        }
        RuleDebugEvaluation {
            rule_id: self.spec.id.clone(),
            state: self.matches(context),
            conditions,
        }
    }

    fn matches_request(&self, context: &RequestReceivedContext) -> MatchState {
        let base = self.conditions.matches_parts(
            &context.request,
            &context.harness,
            &context.session,
            None,
            None,
            None,
            None,
            None,
        );
        match &self.expression {
            Some(expression) => base.and(expression.matches_parts(
                &context.request,
                &context.harness,
                &context.session,
                None,
                None,
                None,
                None,
                None,
            )),
            None => base,
        }
    }

    fn matches_response(&self, context: &ResponseReadyContext) -> MatchState {
        let base = self.conditions.matches_parts(
            &context.request,
            &context.harness,
            &context.session,
            Some(&context.route),
            Some(context.response.status),
            Some(&context.response.headers),
            Some(&context.response.body),
            None,
        );
        match &self.expression {
            Some(expression) => base.and(expression.matches_parts(
                &context.request,
                &context.harness,
                &context.session,
                Some(&context.route),
                Some(context.response.status),
                Some(&context.response.headers),
                Some(&context.response.body),
                None,
            )),
            None => base,
        }
    }

    fn decision(&self) -> AttemptDecision {
        let action = &self.spec.action;
        if action.return_original {
            return AttemptDecision::ReturnOriginal {
                reason: self.spec.name.clone(),
            };
        }
        if let Some(retry) = &action.retry_same_route {
            return AttemptDecision::RetrySameRoute {
                exclude_current_account: retry.exclude_current_account,
                reason: nonempty_reason(&retry.reason, &self.spec.name),
            };
        }
        if let Some(reroute) = &action.reroute {
            let providers = if reroute.providers.is_empty() {
                ProviderConstraint::Any
            } else {
                match reroute.provider_mode {
                    // A nonempty provider list uses `only` by default, matching
                    // the compact TOML generated by the wizard and design example.
                    ProviderModeV1::Any | ProviderModeV1::Only => {
                        ProviderConstraint::Only(reroute.providers.clone())
                    }
                    ProviderModeV1::Prefer => ProviderConstraint::Prefer(reroute.providers.clone()),
                    ProviderModeV1::Exclude => {
                        ProviderConstraint::Exclude(reroute.providers.clone())
                    }
                }
            };
            let target = match (&reroute.model, &reroute.equivalent_class) {
                (Some(model), None) => RouteTarget::Exact {
                    model: model.clone(),
                    providers,
                },
                (None, Some(class)) => RouteTarget::Equivalent {
                    class: class.clone(),
                    providers,
                },
                _ => unreachable!("validated route target"),
            };
            let scope = match reroute.scope {
                RouteScopeKindV1::Request => RouteScope::Request,
                RouteScopeKindV1::Session => RouteScope::Session {
                    ttl_seconds: reroute.ttl_seconds.expect("validated session TTL"),
                },
            };
            return AttemptDecision::Reroute {
                target,
                scope,
                notice: reroute
                    .notice
                    .as_ref()
                    .map(|text| ResponseNotice { text: text.clone() }),
                reason: nonempty_reason(&reroute.reason, &self.spec.name),
            };
        }
        AttemptDecision::Continue
    }
}

fn debug_condition_groups(spec: &MatchConditionsV1) -> Vec<(String, CompiledConditions)> {
    let mut groups = Vec::new();
    let mut push = |name: &str, conditions: MatchConditionsV1| {
        if !conditions.is_empty() {
            groups.push((name.to_owned(), CompiledConditions::compile(&conditions)));
        }
    };

    push(
        "harness",
        MatchConditionsV1 {
            harness_names: spec.harness_names.clone(),
            harness_versions: spec.harness_versions.clone(),
            harness_name_regex: spec.harness_name_regex.clone(),
            harness_version_regex: spec.harness_version_regex.clone(),
            ..Default::default()
        },
    );
    push(
        "model",
        MatchConditionsV1 {
            models: spec.models.clone(),
            model_regex: spec.model_regex.clone(),
            original_models: spec.original_models.clone(),
            current_models: spec.current_models.clone(),
            model_aliases: spec.model_aliases.clone(),
            equivalence_classes: spec.equivalence_classes.clone(),
            ..Default::default()
        },
    );
    push(
        "effort",
        MatchConditionsV1 {
            efforts: spec.efforts.clone(),
            ..Default::default()
        },
    );
    push(
        "provider",
        MatchConditionsV1 {
            providers: spec.providers.clone(),
            provider_regex: spec.provider_regex.clone(),
            exclude_providers: spec.exclude_providers.clone(),
            ..Default::default()
        },
    );
    push(
        "status",
        MatchConditionsV1 {
            status: spec.status.clone(),
            status_regex: spec.status_regex.clone(),
            ..Default::default()
        },
    );
    push(
        "response_headers",
        MatchConditionsV1 {
            response_header_regex: spec.response_header_regex.clone(),
            ..Default::default()
        },
    );
    push(
        "error",
        MatchConditionsV1 {
            error_classes: spec.error_classes.clone(),
            error_kinds: spec.error_kinds.clone(),
            error_codes: spec.error_codes.clone(),
            error_messages: spec.error_messages.clone(),
            ..Default::default()
        },
    );
    push(
        "body",
        MatchConditionsV1 {
            body_contains: spec.body_contains.clone(),
            body_contains_any: spec.body_contains_any.clone(),
            body_regex: spec.body_regex.clone(),
            body_json_equals: spec.body_json_equals.clone(),
            require_complete_body: spec.require_complete_body,
            content_types: spec.content_types.clone(),
            ..Default::default()
        },
    );
    push(
        "attempt",
        MatchConditionsV1 {
            attempt_numbers: spec.attempt_numbers.clone(),
            same_route_accounts_remaining: spec.same_route_accounts_remaining,
            ..Default::default()
        },
    );
    push(
        "session",
        MatchConditionsV1 {
            session_present: spec.session_present,
            stable_session: spec.stable_session,
            ..Default::default()
        },
    );
    groups
}

fn nonempty_reason(reason: &str, fallback: &str) -> String {
    if reason.trim().is_empty() {
        fallback.to_owned()
    } else {
        reason.to_owned()
    }
}

#[derive(Debug, Clone)]
enum CompiledExpression {
    All(Vec<CompiledExpression>),
    Any(Vec<CompiledExpression>),
    Not(Box<CompiledExpression>),
    Conditions(Box<CompiledConditions>),
}

impl CompiledExpression {
    fn compile(expression: &MatchExpressionV1) -> Self {
        match expression {
            MatchExpressionV1::All { all } => Self::All(all.iter().map(Self::compile).collect()),
            MatchExpressionV1::Any { any } => Self::Any(any.iter().map(Self::compile).collect()),
            MatchExpressionV1::Not { not } => Self::Not(Box::new(Self::compile(not))),
            MatchExpressionV1::Conditions { conditions } => {
                Self::Conditions(Box::new(CompiledConditions::compile(conditions)))
            }
        }
    }

    fn matches_attempt(&self, context: &AttemptResultContext) -> MatchState {
        self.matches_parts(
            &context.request,
            &context.harness,
            &context.session,
            Some(&context.route),
            Some(context.outcome.status),
            Some(&context.outcome.headers),
            Some(&context.outcome.body),
            context.outcome.error.as_ref(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn matches_parts(
        &self,
        request: &crate::ClientRequestView,
        harness: &crate::HarnessView,
        session: &crate::SessionView,
        route: Option<&crate::RouteView>,
        status: Option<u16>,
        response_headers: Option<&crate::SafeHeaders>,
        body: Option<&crate::BodyView>,
        error: Option<&crate::ErrorInfo>,
    ) -> MatchState {
        match self {
            Self::All(expressions) => {
                expressions
                    .iter()
                    .fold(MatchState::Matched, |state, expression| {
                        state.and(expression.matches_parts(
                            request,
                            harness,
                            session,
                            route,
                            status,
                            response_headers,
                            body,
                            error,
                        ))
                    })
            }
            Self::Any(expressions) => {
                let mut needs_body = false;
                for expression in expressions {
                    match expression.matches_parts(
                        request,
                        harness,
                        session,
                        route,
                        status,
                        response_headers,
                        body,
                        error,
                    ) {
                        MatchState::Matched => return MatchState::Matched,
                        MatchState::NeedsBody => needs_body = true,
                        MatchState::NotMatched => {}
                    }
                }
                if needs_body {
                    MatchState::NeedsBody
                } else {
                    MatchState::NotMatched
                }
            }
            Self::Not(expression) => expression
                .matches_parts(
                    request,
                    harness,
                    session,
                    route,
                    status,
                    response_headers,
                    body,
                    error,
                )
                .invert(),
            Self::Conditions(conditions) => conditions.matches_parts(
                request,
                harness,
                session,
                route,
                status,
                response_headers,
                body,
                error,
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledConditions {
    harness_names: StringSet,
    harness_versions: VersionSet,
    harness_name_regex: Vec<Regex>,
    harness_version_regex: Vec<Regex>,
    models: StringSet,
    model_regex: Vec<Regex>,
    original_models: StringSet,
    current_models: StringSet,
    model_aliases: StringSet,
    equivalence_classes: StringSet,
    efforts: StringSet,
    providers: StringSet,
    provider_regex: Vec<Regex>,
    exclude_providers: StringSet,
    status: Vec<(u16, u16)>,
    status_regex: Vec<Regex>,
    response_header_regex: Vec<(Regex, Regex)>,
    error_classes: HashSet<ErrorClass>,
    error_kinds: StringSet,
    error_codes: StringSet,
    error_messages: StringSet,
    body_contains_all: Option<AhoCorasick>,
    body_contains_all_count: usize,
    body_contains_any: Option<AhoCorasick>,
    body_regex: Vec<Regex>,
    body_json_equals: Vec<(String, serde_json::Value)>,
    require_complete_body: bool,
    content_types: StringSet,
    attempt_numbers: HashSet<u32>,
    same_route_accounts_remaining: Option<bool>,
    session_present: Option<bool>,
    stable_session: Option<bool>,
    needs_body: bool,
}

impl CompiledConditions {
    fn compile(spec: &MatchConditionsV1) -> Self {
        let mut unique_all = Vec::new();
        let mut seen_all = HashSet::new();
        for phrase in &spec.body_contains {
            if seen_all.insert(phrase.as_str()) {
                unique_all.push(phrase.as_str());
            }
        }
        let body_contains_all = (!unique_all.is_empty())
            .then(|| AhoCorasick::new(&unique_all).expect("validated body phrases"));
        let body_contains_any = (!spec.body_contains_any.is_empty())
            .then(|| AhoCorasick::new(&spec.body_contains_any).expect("validated body phrases"));
        Self {
            harness_names: StringSet::compile(&spec.harness_names),
            harness_versions: VersionSet::compile(&spec.harness_versions),
            harness_name_regex: compile_regexes(&spec.harness_name_regex),
            harness_version_regex: compile_regexes(&spec.harness_version_regex),
            models: StringSet::compile(&spec.models),
            model_regex: compile_regexes(&spec.model_regex),
            original_models: StringSet::compile(&spec.original_models),
            current_models: StringSet::compile(&spec.current_models),
            model_aliases: StringSet::compile(&spec.model_aliases),
            equivalence_classes: StringSet::compile(&spec.equivalence_classes),
            efforts: StringSet::compile(&spec.efforts),
            providers: StringSet::compile(&spec.providers),
            provider_regex: compile_regexes(&spec.provider_regex),
            exclude_providers: StringSet::compile(&spec.exclude_providers),
            status: spec.status.iter().filter_map(parse_status).collect(),
            status_regex: compile_regexes(&spec.status_regex),
            response_header_regex: spec
                .response_header_regex
                .iter()
                .map(|matcher| {
                    (
                        Regex::new(&matcher.key).expect("validated header key regex"),
                        Regex::new(&matcher.value).expect("validated header value regex"),
                    )
                })
                .collect(),
            error_classes: spec.error_classes.iter().copied().collect(),
            error_kinds: StringSet::compile(&spec.error_kinds),
            error_codes: StringSet::compile(&spec.error_codes),
            error_messages: StringSet::compile(&spec.error_messages),
            body_contains_all,
            body_contains_all_count: unique_all.len(),
            body_contains_any,
            body_regex: spec
                .body_regex
                .iter()
                .map(|value| Regex::new(value).expect("validated regex"))
                .collect(),
            body_json_equals: spec
                .body_json_equals
                .iter()
                .map(|matcher| (matcher.pointer.clone(), matcher.value.clone()))
                .collect(),
            require_complete_body: spec.require_complete_body,
            content_types: StringSet::compile(&spec.content_types),
            attempt_numbers: spec.attempt_numbers.iter().copied().collect(),
            same_route_accounts_remaining: spec.same_route_accounts_remaining,
            session_present: spec.session_present,
            stable_session: spec.stable_session,
            needs_body: spec.needs_body(),
        }
    }

    fn matches_attempt(&self, context: &AttemptResultContext) -> MatchState {
        self.matches_parts(
            &context.request,
            &context.harness,
            &context.session,
            Some(&context.route),
            Some(context.outcome.status),
            Some(&context.outcome.headers),
            Some(&context.outcome.body),
            context.outcome.error.as_ref(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn matches_parts(
        &self,
        request: &crate::ClientRequestView,
        harness: &crate::HarnessView,
        session: &crate::SessionView,
        route: Option<&crate::RouteView>,
        status: Option<u16>,
        response_headers: Option<&crate::SafeHeaders>,
        body: Option<&crate::BodyView>,
        error: Option<&crate::ErrorInfo>,
    ) -> MatchState {
        // Cheap scalar and static metadata checks always precede body access.
        if !self.harness_names.matches_optional(harness.name.as_deref())
            || !self
                .harness_versions
                .matches_optional(harness.version.as_deref())
            || !regexes_match_optional(&self.harness_name_regex, harness.name.as_deref())
            || !regexes_match_optional(&self.harness_version_regex, harness.version.as_deref())
            || (!self.models.is_empty()
                && !self.models.matches(&request.original_model)
                && !self.models.matches(&request.current_model))
            || (!self.model_regex.is_empty()
                && !regexes_match(&self.model_regex, &request.original_model)
                && !regexes_match(&self.model_regex, &request.current_model))
            || !self.original_models.matches(&request.original_model)
            || !self.current_models.matches(&request.current_model)
            || (!self.model_aliases.is_empty()
                && !route.is_some_and(|route| {
                    route
                        .selected
                        .aliases
                        .iter()
                        .chain(route.requested.aliases.iter())
                        .any(|alias| self.model_aliases.matches(alias))
                }))
            || (!self.equivalence_classes.is_empty()
                && !route.is_some_and(|route| {
                    route
                        .selected
                        .equivalence_classes
                        .iter()
                        .chain(route.requested.equivalence_classes.iter())
                        .any(|class| self.equivalence_classes.matches(class))
                }))
            || !self.efforts.matches_optional(request_effort(request))
            || (!self.providers.is_empty()
                && !route.is_some_and(|route| self.providers.matches(&route.provider.id)))
            || (!self.provider_regex.is_empty()
                && !route
                    .is_some_and(|route| regexes_match(&self.provider_regex, &route.provider.id)))
            || (!self.exclude_providers.is_empty()
                && route.is_some_and(|route| self.exclude_providers.matches(&route.provider.id)))
            || (!self.status.is_empty()
                && !status.is_some_and(|status| {
                    self.status
                        .iter()
                        .any(|(start, end)| (*start..=*end).contains(&status))
                }))
            || !regexes_match_optional(
                &self.status_regex,
                status.map(|status| status.to_string()).as_deref(),
            )
            || (!self.response_header_regex.is_empty()
                && !response_headers.is_some_and(|headers| {
                    self.response_header_regex
                        .iter()
                        .all(|(key_regex, value_regex)| {
                            headers.iter().any(|(key, values)| {
                                key_regex.is_match(key)
                                    && values.iter().any(|value| value_regex.is_match(value))
                            })
                        })
                }))
            || (!self.error_classes.is_empty()
                && !error.is_some_and(|error| self.error_classes.contains(&error.class)))
            || !self
                .error_kinds
                .matches_optional(error.and_then(|error| error.kind.as_deref()))
            || !self
                .error_codes
                .matches_optional(error.and_then(|error| error.code.as_deref()))
            || !self
                .error_messages
                .matches_optional(error.and_then(|error| error.message.as_deref()))
            || !self
                .content_types
                .matches_content_type(body.and_then(|body| body.content_type.as_deref()))
            || (!self.attempt_numbers.is_empty()
                && !route.is_some_and(|route| self.attempt_numbers.contains(&route.attempt_number)))
            || self.same_route_accounts_remaining.is_some_and(|expected| {
                route.is_none_or(|route| expected != route.same_route_accounts_remaining)
            })
            || self
                .session_present
                .is_some_and(|expected| expected != session.id.is_some())
            || self
                .stable_session
                .is_some_and(|expected| expected != session.has_stable_id())
        {
            return MatchState::NotMatched;
        }

        if !self.needs_body {
            return MatchState::Matched;
        }
        let Some(body) = body else {
            return MatchState::NeedsBody;
        };
        if !body.inspected() && body.text.is_none() && body.json.is_none() {
            return MatchState::NeedsBody;
        }
        if self.require_complete_body && body.truncated {
            return MatchState::NotMatched;
        }
        if self.body_contains_all.is_some()
            || self.body_contains_any.is_some()
            || !self.body_regex.is_empty()
        {
            let Some(text) = body.text.as_deref() else {
                return MatchState::NotMatched;
            };
            if let Some(matcher) = &self.body_contains_all {
                let matched: HashSet<_> = matcher.find_iter(text).map(|m| m.pattern()).collect();
                if matched.len() != self.body_contains_all_count {
                    return MatchState::NotMatched;
                }
            }
            if self
                .body_contains_any
                .as_ref()
                .is_some_and(|matcher| !matcher.is_match(text))
                || self.body_regex.iter().any(|regex| !regex.is_match(text))
            {
                return MatchState::NotMatched;
            }
        }
        if !self.body_json_equals.is_empty() {
            if body.truncated {
                return MatchState::NotMatched;
            }
            let Some(json) = body.json.as_ref() else {
                return MatchState::NotMatched;
            };
            if self
                .body_json_equals
                .iter()
                .any(|(pointer, expected)| json.pointer(pointer) != Some(expected))
            {
                return MatchState::NotMatched;
            }
        }
        MatchState::Matched
    }
}

fn compile_regexes(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("validated matcher regex"))
        .collect()
}

fn regexes_match(regexes: &[Regex], value: &str) -> bool {
    regexes.is_empty() || regexes.iter().any(|regex| regex.is_match(value))
}

fn regexes_match_optional(regexes: &[Regex], value: Option<&str>) -> bool {
    regexes.is_empty() || value.is_some_and(|value| regexes_match(regexes, value))
}

fn request_effort(request: &crate::ClientRequestView) -> Option<&str> {
    let body = request.body.json.as_ref()?;
    [
        "/output_config/effort",
        "/reasoning/effort",
        "/reasoning_effort",
        "/thinking/effort",
        "/generationConfig/thinkingConfig/thinkingLevel",
        "/generation_config/thinking_config/thinking_level",
    ]
    .into_iter()
    .find_map(|pointer| body.pointer(pointer).and_then(serde_json::Value::as_str))
}

#[derive(Debug, Clone, Default)]
struct StringSet {
    patterns: Vec<Regex>,
}

impl StringSet {
    fn compile(values: &[String]) -> Self {
        Self {
            patterns: values
                .iter()
                .map(|value| compile_glob_regex(value).expect("validated glob"))
                .collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    fn matches(&self, value: &str) -> bool {
        self.is_empty() || self.patterns.iter().any(|pattern| pattern.is_match(value))
    }

    fn matches_optional(&self, value: Option<&str>) -> bool {
        self.is_empty() || value.is_some_and(|value| self.matches(value))
    }

    fn matches_content_type(&self, value: Option<&str>) -> bool {
        self.is_empty()
            || value.is_some_and(|value| {
                self.matches(value)
                    || value
                        .split_once(';')
                        .is_some_and(|(mime, _)| self.matches(mime.trim()))
            })
    }
}

#[derive(Debug, Clone)]
enum VersionMatcher {
    Exact(String),
    Requirement(VersionReq),
}

#[derive(Debug, Clone, Default)]
struct VersionSet(Vec<VersionMatcher>);

impl VersionSet {
    fn compile(values: &[String]) -> Self {
        Self(
            values
                .iter()
                .map(|value| {
                    if let Some(requirement) = value.strip_prefix("req:") {
                        VersionMatcher::Requirement(
                            VersionReq::parse(requirement).expect("validated version requirement"),
                        )
                    } else if starts_like_version_requirement(value) {
                        VersionMatcher::Requirement(
                            VersionReq::parse(value).expect("validated version requirement"),
                        )
                    } else {
                        VersionMatcher::Exact(value.clone())
                    }
                })
                .collect(),
        )
    }

    fn matches_optional(&self, value: Option<&str>) -> bool {
        self.0.is_empty()
            || value.is_some_and(|value| {
                self.0.iter().any(|matcher| match matcher {
                    VersionMatcher::Exact(exact) => exact == value,
                    VersionMatcher::Requirement(requirement) => Version::parse(value)
                        .ok()
                        .is_some_and(|version| requirement.matches(&version)),
                })
            })
    }
}

#[derive(Debug, Clone, Default)]
struct RuleIndex {
    by_hook: BTreeMap<HookPoint, Vec<usize>>,
    attempt: AttemptIndex,
}

impl RuleIndex {
    fn build(rules: &[CompiledRule]) -> Self {
        let mut index = Self::default();
        for (position, rule) in rules.iter().enumerate() {
            index
                .by_hook
                .entry(rule.spec.hook)
                .or_default()
                .push(position);
            if rule.spec.hook == HookPoint::AttemptResult {
                index.attempt.insert(position, &rule.spec.when);
            }
        }
        index
    }

    fn attempt_candidates(&self, context: &AttemptResultContext) -> Vec<usize> {
        let hook = self
            .by_hook
            .get(&HookPoint::AttemptResult)
            .cloned()
            .unwrap_or_default();
        if hook.is_empty() {
            return hook;
        }
        let mut candidates: BTreeSet<_> = hook.into_iter().collect();
        intersect(
            &mut candidates,
            self.attempt.provider_candidates(&context.route.provider.id),
        );
        intersect(
            &mut candidates,
            self.attempt.model_candidates([
                context.request.original_model.as_str(),
                context.request.current_model.as_str(),
            ]),
        );
        intersect(
            &mut candidates,
            self.attempt.status_candidates(context.outcome.status),
        );
        // BTreeSet numeric order is compiled priority order because rule positions
        // were assigned after deterministic sorting.
        candidates.into_iter().collect()
    }
}

fn intersect(current: &mut BTreeSet<usize>, eligible: BTreeSet<usize>) {
    current.retain(|value| eligible.contains(value));
}

#[derive(Debug, Clone)]
struct AttemptIndex {
    provider_exact: HashMap<String, BTreeSet<usize>>,
    provider_dynamic: BTreeSet<usize>,
    model_exact: HashMap<String, BTreeSet<usize>>,
    model_dynamic: BTreeSet<usize>,
    status: Vec<BTreeSet<usize>>,
    status_dynamic: BTreeSet<usize>,
}

impl Default for AttemptIndex {
    fn default() -> Self {
        Self {
            provider_exact: HashMap::new(),
            provider_dynamic: BTreeSet::new(),
            model_exact: HashMap::new(),
            model_dynamic: BTreeSet::new(),
            status: (0..600).map(|_| BTreeSet::new()).collect(),
            status_dynamic: BTreeSet::new(),
        }
    }
}

impl AttemptIndex {
    fn insert(&mut self, index: usize, conditions: &MatchConditionsV1) {
        if conditions.providers.is_empty()
            || !conditions.exclude_providers.is_empty()
            || conditions.providers.iter().any(|value| has_glob(value))
        {
            self.provider_dynamic.insert(index);
        } else {
            for provider in &conditions.providers {
                self.provider_exact
                    .entry(provider.to_ascii_lowercase())
                    .or_default()
                    .insert(index);
            }
        }

        let mut model_values = Vec::new();
        model_values.extend(&conditions.models);
        model_values.extend(&conditions.original_models);
        model_values.extend(&conditions.current_models);
        if model_values.is_empty()
            || !conditions.model_aliases.is_empty()
            || !conditions.equivalence_classes.is_empty()
            || model_values.iter().any(|value| has_glob(value))
        {
            self.model_dynamic.insert(index);
        } else {
            for model in model_values {
                self.model_exact
                    .entry(model.to_ascii_lowercase())
                    .or_default()
                    .insert(index);
            }
        }

        if conditions.status.is_empty() {
            self.status_dynamic.insert(index);
        } else {
            for (start, end) in conditions.status.iter().filter_map(parse_status) {
                for status in start..=end {
                    self.status[usize::from(status)].insert(index);
                }
            }
        }
    }

    fn provider_candidates(&self, provider: &str) -> BTreeSet<usize> {
        let mut candidates = self.provider_dynamic.clone();
        if let Some(exact) = self.provider_exact.get(&provider.to_ascii_lowercase()) {
            candidates.extend(exact);
        }
        candidates
    }

    fn model_candidates<'a>(&self, models: impl IntoIterator<Item = &'a str>) -> BTreeSet<usize> {
        let mut candidates = self.model_dynamic.clone();
        for model in models {
            if let Some(exact) = self.model_exact.get(&model.to_ascii_lowercase()) {
                candidates.extend(exact);
            }
        }
        candidates
    }

    fn status_candidates(&self, status: u16) -> BTreeSet<usize> {
        let mut candidates = self.status_dynamic.clone();
        if let Some(exact) = self.status.get(usize::from(status)) {
            candidates.extend(exact);
        }
        candidates
    }
}

fn has_glob(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActionSpecV1, AttemptOutcome, BodyView, Capability, ClientFormat, ClientRequestView,
        ErrorInfo, HarnessView, HttpResponseView, JsonBodyView, JsonPointerEqualsV1,
        ModelCapabilities, ModelRef, ProviderModeV1, ProviderView, RerouteActionSpecV1,
        RouteScopeKindV1, RouteView, SafeHeaders, SessionIdSource, SessionView, StatusMatcherSpec,
    };
    use serde_json::json;

    fn context() -> AttemptResultContext {
        let model = ModelRef {
            provider: "anthropic".into(),
            id: "claude-fable-5".into(),
            aliases: vec!["fable-five".into(), "fable".into()],
            equivalence_classes: vec!["premium-reasoning".into()],
            capabilities: ModelCapabilities {
                tools: true,
                portable_history: true,
                ..Default::default()
            },
        };
        let text = r#"{"error":{"type":"overloaded_error","code":"capacity","message":"model is currently overloaded"}}"#;
        AttemptResultContext {
            request: ClientRequestView {
                trace_id: "trace".into(),
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
                version: Some("1.7.2".into()),
            },
            session: SessionView {
                id: Some("session".into()),
                run_id: Some("run".into()),
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
                attempt_number: 2,
                same_route_accounts_remaining: false,
            },
            outcome: AttemptOutcome {
                status: 529,
                headers: SafeHeaders::default(),
                body: BodyView {
                    content_type: Some("application/json; charset=utf-8".into()),
                    size_bytes: Some(text.len() as u64),
                    text: Some(text.into()),
                    json: serde_json::from_str(text).ok(),
                    truncated: false,
                    inspected_bytes: text.len(),
                },
                error: Some(ErrorInfo {
                    class: ErrorClass::Capacity,
                    kind: Some("overloaded_error".into()),
                    code: Some("capacity".into()),
                    message: Some("model is currently overloaded".into()),
                }),
                timing: Default::default(),
            },
        }
    }

    fn continue_rule(id: &str, priority: i32, when: MatchConditionsV1) -> RuleSpecV1 {
        RuleSpecV1 {
            id: id.into(),
            name: id.into(),
            description: None,
            enabled: true,
            debug: false,
            priority,
            hook: HookPoint::AttemptResult,
            capabilities: if when.needs_body() {
                vec![Capability::AttemptReadErrorBody]
            } else {
                Vec::new()
            },
            when,
            expression: None,
            action: ActionSpecV1 {
                continue_action: true,
                ..Default::default()
            },
        }
    }

    fn reroute_rule(id: &str, priority: i32, target: &str) -> RuleSpecV1 {
        RuleSpecV1 {
            id: id.into(),
            name: id.into(),
            description: None,
            enabled: true,
            debug: false,
            priority,
            hook: HookPoint::AttemptResult,
            capabilities: vec![Capability::RouteOverride],
            when: MatchConditionsV1 {
                status: vec![StatusMatcherSpec::RangeOrClass("5xx".into())],
                ..Default::default()
            },
            expression: None,
            action: ActionSpecV1 {
                reroute: Some(RerouteActionSpecV1 {
                    model: Some(target.into()),
                    equivalent_class: None,
                    providers: vec!["openai".into()],
                    provider_mode: ProviderModeV1::Only,
                    scope: RouteScopeKindV1::Request,
                    ttl_seconds: None,
                    notice: None,
                    effort: None,
                    reason: id.into(),
                    max_attempts: None,
                    required_capabilities: Default::default(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn debug_rules_report_grouped_match_reasons_and_body_work() {
        let mut rule = continue_rule(
            "debug-rule",
            0,
            MatchConditionsV1 {
                model_regex: vec!["^claude-fable-5$".into()],
                provider_regex: vec!["^anthropic$".into()],
                status_regex: vec!["^529$".into()],
                body_regex: vec!["currently overloaded".into()],
                stable_session: Some(true),
                ..Default::default()
            },
        );
        rule.debug = true;
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule],
        })
        .unwrap();

        let evaluation = engine.evaluate_debug_attempt(&context());
        assert_eq!(evaluation.len(), 1);
        assert_eq!(evaluation[0].state, MatchState::Matched);
        assert_eq!(
            evaluation[0]
                .conditions
                .iter()
                .map(|condition| (condition.group.as_str(), condition.state))
                .collect::<Vec<_>>(),
            vec![
                ("model", MatchState::Matched),
                ("provider", MatchState::Matched),
                ("status", MatchState::Matched),
                ("body", MatchState::Matched),
                ("session", MatchState::Matched),
            ]
        );

        let mut head_only = context();
        head_only.outcome.body = BodyView {
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let plan = engine.debug_inspection_plan(&head_only);
        assert!(plan.needs_body);
        assert_eq!(plan.candidate_rule_ids, ["debug-rule"]);

        head_only.request.original_model = "other".into();
        head_only.request.current_model = "other".into();
        assert!(!engine.debug_inspection_plan(&head_only).needs_body);
        let evaluation = engine.evaluate_debug_attempt(&head_only);
        assert_eq!(evaluation[0].state, MatchState::NotMatched);
        assert_eq!(
            evaluation[0]
                .conditions
                .iter()
                .find(|condition| condition.group == "model")
                .map(|condition| condition.state),
            Some(MatchState::NotMatched)
        );
    }

    #[test]
    fn all_any_and_not_expressions_compose() {
        let mut rule = continue_rule(
            "expression",
            0,
            MatchConditionsV1 {
                status: vec![StatusMatcherSpec::RangeOrClass("500-599".into())],
                ..Default::default()
            },
        );
        rule.expression = Some(MatchExpressionV1::All {
            all: vec![
                MatchExpressionV1::Any {
                    any: vec![
                        MatchExpressionV1::Conditions {
                            conditions: Box::new(MatchConditionsV1 {
                                providers: vec!["openai".into()],
                                ..Default::default()
                            }),
                        },
                        MatchExpressionV1::Conditions {
                            conditions: Box::new(MatchConditionsV1 {
                                models: vec!["*fable-*".into()],
                                ..Default::default()
                            }),
                        },
                    ],
                },
                MatchExpressionV1::Not {
                    not: Box::new(MatchExpressionV1::Conditions {
                        conditions: Box::new(MatchConditionsV1 {
                            error_classes: vec![ErrorClass::Auth],
                            ..Default::default()
                        }),
                    }),
                },
            ],
        });
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule],
        })
        .unwrap();
        assert_eq!(
            engine.evaluate_attempt(&context()).records[0].state,
            MatchState::Matched
        );
    }

    #[test]
    fn model_exact_glob_alias_and_equivalence_matchers_all_work() {
        let rules = vec![
            continue_rule(
                "exact-list",
                4,
                MatchConditionsV1 {
                    models: vec!["other".into(), "claude-fable-5".into()],
                    ..Default::default()
                },
            ),
            continue_rule(
                "glob",
                3,
                MatchConditionsV1 {
                    current_models: vec!["claude-*-5".into()],
                    ..Default::default()
                },
            ),
            continue_rule(
                "alias",
                2,
                MatchConditionsV1 {
                    model_aliases: vec!["fable-five".into()],
                    ..Default::default()
                },
            ),
            continue_rule(
                "equivalence",
                1,
                MatchConditionsV1 {
                    equivalence_classes: vec!["premium-*".into()],
                    ..Default::default()
                },
            ),
        ];
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules,
        })
        .unwrap();
        let matched: Vec<_> = engine
            .evaluate_attempt(&context())
            .records
            .into_iter()
            .filter(|record| record.state == MatchState::Matched)
            .map(|record| record.rule_id)
            .collect();
        assert_eq!(matched, ["exact-list", "glob", "alias", "equivalence"]);
    }

    #[test]
    fn harness_exact_semver_requirement_and_unknown_exact_are_explicit() {
        let rules = vec![
            continue_rule(
                "exact-version",
                2,
                MatchConditionsV1 {
                    harness_versions: vec!["1.7.2".into()],
                    ..Default::default()
                },
            ),
            continue_rule(
                "version-requirement",
                1,
                MatchConditionsV1 {
                    harness_versions: vec!["req:>=1.7, <2".into()],
                    ..Default::default()
                },
            ),
        ];
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules,
        })
        .unwrap();
        assert_eq!(engine.evaluate_attempt(&context()).records.len(), 2);

        let mut unknown = context();
        unknown.harness.version = Some("nightly-alex".into());
        let exact_unknown = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![continue_rule(
                "unknown-exact",
                0,
                MatchConditionsV1 {
                    harness_versions: vec!["nightly-alex".into()],
                    ..Default::default()
                },
            )],
        })
        .unwrap();
        assert_eq!(
            exact_unknown.evaluate_attempt(&unknown).records[0].state,
            MatchState::Matched
        );
    }

    #[test]
    fn status_error_and_content_type_fields_are_anded() {
        let rule = continue_rule(
            "error-fields",
            0,
            MatchConditionsV1 {
                status: vec![
                    StatusMatcherSpec::Exact(429),
                    StatusMatcherSpec::RangeOrClass("500-599".into()),
                ],
                error_classes: vec![ErrorClass::Capacity],
                error_kinds: vec!["overloaded_*".into()],
                error_codes: vec!["capacity".into()],
                error_messages: vec!["*currently overloaded".into()],
                content_types: vec!["application/json".into()],
                attempt_numbers: vec![2],
                same_route_accounts_remaining: Some(false),
                stable_session: Some(true),
                ..Default::default()
            },
        );
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule],
        })
        .unwrap();
        assert_eq!(
            engine.evaluate_attempt(&context()).records[0].state,
            MatchState::Matched
        );

        let mut wrong = context();
        wrong.outcome.error.as_mut().unwrap().code = Some("other".into());
        assert_eq!(
            engine.evaluate_attempt(&wrong).records[0].state,
            MatchState::NotMatched
        );
    }

    #[test]
    fn body_inspection_plans_after_cheap_prefilters_and_honors_truncation() {
        let mut rule = continue_rule(
            "body",
            0,
            MatchConditionsV1 {
                providers: vec!["anthropic".into()],
                status: vec![StatusMatcherSpec::RangeOrClass("5xx".into())],
                body_contains: vec!["model".into(), "overloaded".into()],
                body_contains_any: vec!["currently".into(), "unavailable".into()],
                body_regex: vec!["overload(ed|ing)".into()],
                ..Default::default()
            },
        );
        let mut head = context();
        head.outcome.body.text = None;
        head.outcome.body.json = None;
        head.outcome.body.size_bytes = None;
        head.outcome.body.inspected_bytes = 0;
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule.clone()],
        })
        .unwrap();
        assert!(engine.inspection_plan(&head).needs_body);
        head.route.provider.id = "openai".into();
        assert!(!engine.inspection_plan(&head).needs_body);

        let mut truncated = context();
        truncated.outcome.body.truncated = true;
        assert_eq!(
            engine.evaluate_attempt(&truncated).records[0].state,
            MatchState::Matched,
            "prefix phrases may match a truncated body"
        );
        rule.when.require_complete_body = true;
        let complete_engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule],
        })
        .unwrap();
        assert_eq!(
            complete_engine.evaluate_attempt(&truncated).records[0].state,
            MatchState::NotMatched
        );
    }

    #[test]
    fn json_pointer_matching_requires_complete_json() {
        let rule = continue_rule(
            "json",
            0,
            MatchConditionsV1 {
                body_json_equals: vec![JsonPointerEqualsV1 {
                    pointer: "/error/type".into(),
                    value: json!("overloaded_error"),
                }],
                ..Default::default()
            },
        );
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![rule],
        })
        .unwrap();
        assert_eq!(
            engine.evaluate_attempt(&context()).records[0].state,
            MatchState::Matched
        );
        let mut truncated = context();
        truncated.outcome.body.truncated = true;
        assert_eq!(
            engine.evaluate_attempt(&truncated).records[0].state,
            MatchState::NotMatched
        );
    }

    #[test]
    fn priority_then_id_order_is_deterministic_and_first_terminal_wins() {
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![
                reroute_rule("z-low", 5, "low"),
                reroute_rule("z-tie", 10, "z"),
                reroute_rule("a-tie", 10, "a"),
            ],
        })
        .unwrap();
        let result = engine.evaluate_attempt(&context());
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].rule_id, "a-tie");
        assert!(matches!(
            result.decision,
            AttemptDecision::Reroute {
                target: RouteTarget::Exact { ref model, .. },
                ..
            } if model == "a"
        ));
    }

    #[test]
    fn json_and_toml_round_trip_preserve_the_public_rule_schema() {
        let rule_set = RuleSetV1 {
            api_version: 1,
            rules: vec![crate::fable_to_sol_rule()],
        };
        let json = serde_json::to_string(&rule_set).unwrap();
        let from_json: RuleSetV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(from_json, rule_set);
        let toml = toml::to_string(&rule_set).unwrap();
        let from_toml: RuleSetV1 = toml::from_str(&toml).unwrap();
        assert_eq!(from_toml, rule_set);
    }

    #[test]
    fn request_and_response_patches_accumulate_in_priority_order() {
        let request_rule = |id: &str, priority, value: &str| RuleSpecV1 {
            id: id.into(),
            name: id.into(),
            description: None,
            enabled: true,
            debug: false,
            priority,
            hook: HookPoint::RequestReceived,
            capabilities: vec![Capability::RequestPatch],
            when: MatchConditionsV1 {
                harness_names: vec!["claude".into()],
                ..Default::default()
            },
            expression: None,
            action: ActionSpecV1 {
                request_headers: vec![HeaderPatch::Append {
                    name: "x-alex-label".into(),
                    value: value.into(),
                }],
                ..Default::default()
            },
        };
        let response_rule = |id: &str, priority, text: &str| RuleSpecV1 {
            id: id.into(),
            name: id.into(),
            description: None,
            enabled: true,
            debug: false,
            priority,
            hook: HookPoint::ResponseReady,
            capabilities: vec![Capability::ResponsePatch],
            when: MatchConditionsV1 {
                status: vec![StatusMatcherSpec::RangeOrClass("5xx".into())],
                ..Default::default()
            },
            expression: None,
            action: ActionSpecV1 {
                response_patches: vec![ResponsePatch::PrependAssistantText(text.into())],
                ..Default::default()
            },
        };
        let engine = CompiledRuleSetV1::compile(RuleSetV1 {
            api_version: 1,
            rules: vec![
                request_rule("request-low", 1, "low"),
                request_rule("request-high", 2, "high"),
                response_rule("response-low", 1, "low"),
                response_rule("response-high", 2, "high"),
            ],
        })
        .unwrap();
        let attempt = context();
        let request_result = engine.evaluate_request_received(&RequestReceivedContext {
            request: attempt.request.clone(),
            harness: attempt.harness.clone(),
            session: attempt.session.clone(),
        });
        let request_values: Vec<_> = request_result
            .request_header_patches
            .iter()
            .map(|patch| match patch {
                HeaderPatch::Append { value, .. } => value.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(request_values, ["high", "low"]);

        let response_result = engine.evaluate_response_ready(&ResponseReadyContext {
            request: attempt.request,
            harness: attempt.harness,
            session: attempt.session,
            route: attempt.route,
            response: HttpResponseView {
                status: 529,
                headers: SafeHeaders::default(),
                body: attempt.outcome.body,
            },
        });
        let response_values: Vec<_> = response_result
            .response_patches
            .iter()
            .map(|patch| match patch {
                ResponsePatch::PrependAssistantText(value) => value.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(response_values, ["high", "low"]);
    }
}
