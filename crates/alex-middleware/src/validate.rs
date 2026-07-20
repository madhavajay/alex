use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use semver::VersionReq;
use serde::{Deserialize, Serialize};

use crate::{
    validate_header_patch, Capability, HeaderPatch, HookPoint, MatchConditionsV1,
    MatchExpressionV1, ModelCapabilities, ProviderModeV1, ResponsePatch, RouteScopeKindV1,
    RuleSetV1, RuleSpecV1, StatusMatcherSpec, API_VERSION_V1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationErrorCode {
    UnsupportedApiVersion,
    DuplicateRuleId,
    InvalidRuleId,
    EmptyName,
    EmptyMatcher,
    EmptyAction,
    MultipleTerminalActions,
    StageActionMismatch,
    InvalidStatusMatcher,
    InvalidGlob,
    InvalidVersionRequirement,
    InvalidRegex,
    EmptyBodyPattern,
    InvalidJsonPointer,
    ReservedHeader,
    MissingCapability,
    InvalidRouteTarget,
    InvalidProviderConstraint,
    InvalidSessionScope,
    AttemptBudgetExceeded,
    DuplicateProvider,
    UnknownTargetModel,
    ModelProviderContradiction,
    TargetCapabilityMismatch,
    EmptyExpression,
    ExpressionTooDeep,
    OversizedOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: ValidationErrorCode,
    pub path: String,
    pub message: String,
}

impl ValidationError {
    fn new(code: ValidationErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOptions {
    pub max_attempts: u32,
    pub max_expression_depth: usize,
    pub max_notice_bytes: usize,
    pub max_patch_value_bytes: usize,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_expression_depth: 32,
            max_notice_bytes: 8 * 1024,
            max_patch_value_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ValidationCatalog {
    pub models: BTreeMap<String, CatalogModel>,
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub provider: String,
    pub capabilities: ModelCapabilities,
}

impl ValidationCatalog {
    pub fn insert(
        &mut self,
        model: impl Into<String>,
        provider: impl Into<String>,
        capabilities: ModelCapabilities,
    ) {
        self.models.insert(
            model.into(),
            CatalogModel {
                provider: provider.into(),
                capabilities,
            },
        );
    }
}

pub fn validate_rule_set(
    rule_set: &RuleSetV1,
    options: &ValidationOptions,
    catalog: Option<&ValidationCatalog>,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if rule_set.api_version != API_VERSION_V1 {
        errors.push(ValidationError::new(
            ValidationErrorCode::UnsupportedApiVersion,
            "api_version",
            format!(
                "middleware API version {} is unsupported; expected {API_VERSION_V1}",
                rule_set.api_version
            ),
        ));
        return errors;
    }

    let mut ids = HashSet::new();
    for (index, rule) in rule_set.rules.iter().enumerate() {
        let path = format!("rules[{index}]");
        if !ids.insert(rule.id.clone()) {
            errors.push(ValidationError::new(
                ValidationErrorCode::DuplicateRuleId,
                format!("{path}.id"),
                format!("duplicate middleware ID {:?}", rule.id),
            ));
        }
        validate_rule(rule, &path, options, catalog, &mut errors);
    }
    errors
}

fn validate_rule(
    rule: &RuleSpecV1,
    path: &str,
    options: &ValidationOptions,
    catalog: Option<&ValidationCatalog>,
    errors: &mut Vec<ValidationError>,
) {
    if rule.id.is_empty()
        || rule.id.len() > 128
        || !rule
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        errors.push(ValidationError::new(
            ValidationErrorCode::InvalidRuleId,
            format!("{path}.id"),
            "rule ID must contain only letters, digits, '.', '-', or '_'",
        ));
    }
    if rule.name.trim().is_empty() {
        errors.push(ValidationError::new(
            ValidationErrorCode::EmptyName,
            format!("{path}.name"),
            "rule name must not be empty",
        ));
    }
    if rule.when.is_empty() && rule.expression.is_none() {
        errors.push(ValidationError::new(
            ValidationErrorCode::EmptyMatcher,
            format!("{path}.when"),
            "rule must have at least one matcher",
        ));
    }
    if !rule.action.has_any_action() {
        errors.push(ValidationError::new(
            ValidationErrorCode::EmptyAction,
            format!("{path}.then"),
            "rule must have at least one action",
        ));
    }
    if rule.action.terminal_action_count() > 1 {
        errors.push(ValidationError::new(
            ValidationErrorCode::MultipleTerminalActions,
            format!("{path}.then"),
            "only one return, retry, or reroute action is allowed",
        ));
    }
    validate_conditions(&rule.when, &format!("{path}.when"), errors);
    if let Some(expression) = &rule.expression {
        validate_expression(
            expression,
            &format!("{path}.expression"),
            1,
            options,
            errors,
        );
    }
    validate_action(rule, path, options, catalog, errors);
}

fn validate_conditions(
    conditions: &MatchConditionsV1,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    for (index, status) in conditions.status.iter().enumerate() {
        if parse_status(status).is_none() {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidStatusMatcher,
                format!("{path}.status[{index}]"),
                format!("invalid HTTP status matcher {status:?}"),
            ));
        }
    }
    for (field, patterns) in [
        ("harness_names", &conditions.harness_names),
        ("models", &conditions.models),
        ("original_models", &conditions.original_models),
        ("current_models", &conditions.current_models),
        ("model_aliases", &conditions.model_aliases),
        ("equivalence_classes", &conditions.equivalence_classes),
        ("providers", &conditions.providers),
        ("exclude_providers", &conditions.exclude_providers),
        ("error_kinds", &conditions.error_kinds),
        ("error_codes", &conditions.error_codes),
        ("error_messages", &conditions.error_messages),
        ("content_types", &conditions.content_types),
    ] {
        for (index, pattern) in patterns.iter().enumerate() {
            if let Err(message) = compile_glob_regex(pattern) {
                errors.push(ValidationError::new(
                    ValidationErrorCode::InvalidGlob,
                    format!("{path}.{field}[{index}]"),
                    message,
                ));
            }
        }
    }
    for (index, version) in conditions.harness_versions.iter().enumerate() {
        if let Some(requirement) = version.strip_prefix("req:") {
            if let Err(error) = VersionReq::parse(requirement) {
                errors.push(ValidationError::new(
                    ValidationErrorCode::InvalidVersionRequirement,
                    format!("{path}.harness_versions[{index}]"),
                    error.to_string(),
                ));
            }
        } else if starts_like_version_requirement(version) && VersionReq::parse(version).is_err() {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidVersionRequirement,
                format!("{path}.harness_versions[{index}]"),
                format!("invalid semantic-version requirement {version:?}"),
            ));
        }
    }
    for (index, expression) in conditions.body_regex.iter().enumerate() {
        if let Err(error) = Regex::new(expression) {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidRegex,
                format!("{path}.body_regex[{index}]"),
                error.to_string(),
            ));
        }
    }
    for (field, phrases) in [
        ("body_contains", &conditions.body_contains),
        ("body_contains_any", &conditions.body_contains_any),
    ] {
        for (index, phrase) in phrases.iter().enumerate() {
            if phrase.is_empty() {
                errors.push(ValidationError::new(
                    ValidationErrorCode::EmptyBodyPattern,
                    format!("{path}.{field}[{index}]"),
                    "body phrase must not be empty",
                ));
            }
        }
    }
    for (index, json_match) in conditions.body_json_equals.iter().enumerate() {
        if !valid_json_pointer(&json_match.pointer) {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidJsonPointer,
                format!("{path}.body_json_equals[{index}].pointer"),
                "JSON pointer must start with '/' and use valid '~0'/'~1' escapes",
            ));
        }
    }
}

fn validate_expression(
    expression: &MatchExpressionV1,
    path: &str,
    depth: usize,
    options: &ValidationOptions,
    errors: &mut Vec<ValidationError>,
) {
    if depth > options.max_expression_depth {
        errors.push(ValidationError::new(
            ValidationErrorCode::ExpressionTooDeep,
            path,
            format!(
                "expression exceeds maximum depth {}",
                options.max_expression_depth
            ),
        ));
        return;
    }
    match expression {
        MatchExpressionV1::All { all } => {
            if all.is_empty() {
                errors.push(ValidationError::new(
                    ValidationErrorCode::EmptyExpression,
                    path,
                    "all expression must contain at least one child",
                ));
            }
            for (index, child) in all.iter().enumerate() {
                validate_expression(
                    child,
                    &format!("{path}.all[{index}]"),
                    depth + 1,
                    options,
                    errors,
                );
            }
        }
        MatchExpressionV1::Any { any } => {
            if any.is_empty() {
                errors.push(ValidationError::new(
                    ValidationErrorCode::EmptyExpression,
                    path,
                    "any expression must contain at least one child",
                ));
            }
            for (index, child) in any.iter().enumerate() {
                validate_expression(
                    child,
                    &format!("{path}.any[{index}]"),
                    depth + 1,
                    options,
                    errors,
                );
            }
        }
        MatchExpressionV1::Not { not } => {
            validate_expression(not, &format!("{path}.not"), depth + 1, options, errors)
        }
        MatchExpressionV1::Conditions { conditions } => {
            if conditions.is_empty() {
                errors.push(ValidationError::new(
                    ValidationErrorCode::EmptyExpression,
                    format!("{path}.conditions"),
                    "conditions expression must contain a matcher",
                ));
            }
            validate_conditions(conditions, &format!("{path}.conditions"), errors);
        }
    }
}

fn validate_action(
    rule: &RuleSpecV1,
    path: &str,
    options: &ValidationOptions,
    catalog: Option<&ValidationCatalog>,
    errors: &mut Vec<ValidationError>,
) {
    let action = &rule.action;
    if rule.hook == HookPoint::RequestReceived
        && (conditions_need_route_or_attempt(&rule.when)
            || rule
                .expression
                .as_ref()
                .is_some_and(expression_needs_route_or_attempt))
    {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.when"),
            "provider, route, status, error, content-type, and attempt matchers are unavailable at request_received",
        ));
    }
    if rule.hook == HookPoint::ResponseReady
        && (conditions_need_error_or_attempt(&rule.when)
            || rule
                .expression
                .as_ref()
                .is_some_and(expression_needs_error_or_attempt))
    {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.when"),
            "normalized upstream error and attempt-account matchers are unavailable at response_ready",
        ));
    }
    if action.terminal_action_count() > 0 && rule.hook != HookPoint::AttemptResult {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.then"),
            "return, retry, and reroute are only legal at attempt_result",
        ));
    }
    if !action.request_headers.is_empty() && rule.hook != HookPoint::RequestReceived {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.then.request_headers"),
            "request header patches are only legal at request_received",
        ));
    }
    if !action.response_patches.is_empty() && rule.hook != HookPoint::ResponseReady {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.then.response_patches"),
            "response patches are only legal at response_ready",
        ));
    }
    if matches!(
        rule.hook,
        HookPoint::RequestReceived | HookPoint::RoutePlanned | HookPoint::TraceFinalized
    ) && (rule.when.needs_body()
        || rule
            .expression
            .as_ref()
            .is_some_and(MatchExpressionV1::needs_body))
    {
        errors.push(ValidationError::new(
            ValidationErrorCode::StageActionMismatch,
            format!("{path}.when"),
            "body matchers are unavailable at this hook",
        ));
    }

    for (index, patch) in action.request_headers.iter().enumerate() {
        let result = match patch {
            HeaderPatch::Set { name, value } | HeaderPatch::Append { name, value } => {
                validate_header_patch(name, Some(value))
            }
            HeaderPatch::Remove { name } => validate_header_patch(name, None),
        };
        if let Err(error) = result {
            errors.push(ValidationError::new(
                ValidationErrorCode::ReservedHeader,
                format!("{path}.then.request_headers[{index}]"),
                error.to_string(),
            ));
        }
    }
    for (patch_index, patch) in action.response_patches.iter().enumerate() {
        match patch {
            ResponsePatch::Headers(headers) => {
                for (header_index, header) in headers.iter().enumerate() {
                    let result = match header {
                        HeaderPatch::Set { name, value } | HeaderPatch::Append { name, value } => {
                            validate_header_patch(name, Some(value))
                        }
                        HeaderPatch::Remove { name } => validate_header_patch(name, None),
                    };
                    if let Err(error) = result {
                        errors.push(ValidationError::new(
                            ValidationErrorCode::ReservedHeader,
                            format!(
                                "{path}.then.response_patches[{patch_index}].headers[{header_index}]"
                            ),
                            error.to_string(),
                        ));
                    }
                }
            }
            ResponsePatch::PrependAssistantText(text)
            | ResponsePatch::AppendAssistantText(text) => {
                if text.len() > options.max_patch_value_bytes {
                    errors.push(ValidationError::new(
                        ValidationErrorCode::OversizedOutput,
                        format!("{path}.then.response_patches[{patch_index}]"),
                        "response text patch is too large",
                    ));
                }
            }
            ResponsePatch::JsonPatch(operations) => {
                for (operation_index, operation) in operations.iter().enumerate() {
                    if !valid_json_pointer(&operation.path)
                        || operation
                            .from
                            .as_deref()
                            .is_some_and(|from| !valid_json_pointer(from))
                    {
                        errors.push(ValidationError::new(
                            ValidationErrorCode::InvalidJsonPointer,
                            format!(
                                "{path}.then.response_patches[{patch_index}][{operation_index}]"
                            ),
                            "JSON Patch path is not a valid JSON pointer",
                        ));
                    }
                    if operation
                        .value
                        .as_ref()
                        .and_then(|value| serde_json::to_vec(value).ok())
                        .is_some_and(|value| value.len() > options.max_patch_value_bytes)
                    {
                        errors.push(ValidationError::new(
                            ValidationErrorCode::OversizedOutput,
                            format!(
                                "{path}.then.response_patches[{patch_index}][{operation_index}]"
                            ),
                            "JSON Patch value is too large",
                        ));
                    }
                }
            }
        }
    }

    let capabilities: HashSet<_> = rule.capabilities.iter().copied().collect();
    let requires_body = rule.when.needs_body()
        || rule
            .expression
            .as_ref()
            .is_some_and(MatchExpressionV1::needs_body);
    if requires_body {
        require_capability(
            &capabilities,
            Capability::AttemptReadErrorBody,
            path,
            errors,
        );
    }
    if action.retry_same_route.is_some() || action.reroute.is_some() {
        require_capability(&capabilities, Capability::RouteOverride, path, errors);
    }
    if !action.request_headers.is_empty() {
        require_capability(&capabilities, Capability::RequestPatch, path, errors);
    }
    if !action.response_patches.is_empty() {
        require_capability(&capabilities, Capability::ResponsePatch, path, errors);
    }

    if let Some(retry) = &action.retry_same_route {
        validate_attempt_limit(retry.max_attempts, path, options, errors);
    }
    if let Some(reroute) = &action.reroute {
        validate_attempt_limit(reroute.max_attempts, path, options, errors);
        let target_count =
            usize::from(reroute.model.is_some()) + usize::from(reroute.equivalent_class.is_some());
        if target_count != 1 {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidRouteTarget,
                format!("{path}.then.reroute"),
                "reroute requires exactly one of model or equivalent_class",
            ));
        }
        if reroute.provider_mode != ProviderModeV1::Any && reroute.providers.is_empty() {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidProviderConstraint,
                format!("{path}.then.reroute.providers"),
                "only/prefer/exclude provider mode requires at least one provider",
            ));
        }
        let mut providers = HashSet::new();
        for provider in &reroute.providers {
            if provider.trim().is_empty() || !providers.insert(provider.to_ascii_lowercase()) {
                errors.push(ValidationError::new(
                    if provider.trim().is_empty() {
                        ValidationErrorCode::InvalidProviderConstraint
                    } else {
                        ValidationErrorCode::DuplicateProvider
                    },
                    format!("{path}.then.reroute.providers"),
                    format!("invalid or duplicate provider {provider:?}"),
                ));
            }
        }
        if reroute.scope == RouteScopeKindV1::Session {
            require_capability(&capabilities, Capability::SessionPin, path, errors);
            if reroute.ttl_seconds == Some(0) || reroute.ttl_seconds.is_none() {
                errors.push(ValidationError::new(
                    ValidationErrorCode::InvalidSessionScope,
                    format!("{path}.then.reroute.ttl_seconds"),
                    "session reroute requires a positive ttl_seconds",
                ));
            }
        } else if reroute.ttl_seconds.is_some() {
            errors.push(ValidationError::new(
                ValidationErrorCode::InvalidSessionScope,
                format!("{path}.then.reroute.ttl_seconds"),
                "ttl_seconds is only valid for session scope",
            ));
        }
        if let Some(notice) = &reroute.notice {
            require_capability(&capabilities, Capability::ResponsePrependText, path, errors);
            if notice.len() > options.max_notice_bytes {
                errors.push(ValidationError::new(
                    ValidationErrorCode::OversizedOutput,
                    format!("{path}.then.reroute.notice"),
                    "response notice is too large",
                ));
            }
        }
        if let (Some(catalog), Some(model)) = (catalog, reroute.model.as_ref()) {
            match catalog.models.get(model) {
                None => errors.push(ValidationError::new(
                    ValidationErrorCode::UnknownTargetModel,
                    format!("{path}.then.reroute.model"),
                    format!("target model {model:?} is not in the model catalog"),
                )),
                Some(target) => {
                    if reroute.provider_mode == ProviderModeV1::Only
                        && !reroute
                            .providers
                            .iter()
                            .any(|provider| provider.eq_ignore_ascii_case(&target.provider))
                    {
                        errors.push(ValidationError::new(
                            ValidationErrorCode::ModelProviderContradiction,
                            format!("{path}.then.reroute.providers"),
                            format!(
                                "model {model:?} belongs to provider {:?}, which is excluded by the only-list",
                                target.provider
                            ),
                        ));
                    }
                    let required = &reroute.required_capabilities;
                    if (required.tools && !target.capabilities.tools)
                        || (required.vision && !target.capabilities.vision)
                        || (required.reasoning && !target.capabilities.reasoning)
                        || (required.portable_history && !target.capabilities.portable_history)
                    {
                        errors.push(ValidationError::new(
                            ValidationErrorCode::TargetCapabilityMismatch,
                            format!("{path}.then.reroute.required_capabilities"),
                            format!("target model {model:?} lacks required capabilities"),
                        ));
                    }
                }
            }
        }
    }
}

fn conditions_need_route_or_attempt(conditions: &MatchConditionsV1) -> bool {
    !conditions.model_aliases.is_empty()
        || !conditions.equivalence_classes.is_empty()
        || !conditions.providers.is_empty()
        || !conditions.exclude_providers.is_empty()
        || !conditions.status.is_empty()
        || conditions_need_error_or_attempt(conditions)
        || !conditions.content_types.is_empty()
}

fn conditions_need_error_or_attempt(conditions: &MatchConditionsV1) -> bool {
    !conditions.error_classes.is_empty()
        || !conditions.error_kinds.is_empty()
        || !conditions.error_codes.is_empty()
        || !conditions.error_messages.is_empty()
        || !conditions.attempt_numbers.is_empty()
        || conditions.same_route_accounts_remaining.is_some()
}

fn expression_needs_route_or_attempt(expression: &MatchExpressionV1) -> bool {
    match expression {
        MatchExpressionV1::All { all } => all.iter().any(expression_needs_route_or_attempt),
        MatchExpressionV1::Any { any } => any.iter().any(expression_needs_route_or_attempt),
        MatchExpressionV1::Not { not } => expression_needs_route_or_attempt(not),
        MatchExpressionV1::Conditions { conditions } => {
            conditions_need_route_or_attempt(conditions)
        }
    }
}

fn expression_needs_error_or_attempt(expression: &MatchExpressionV1) -> bool {
    match expression {
        MatchExpressionV1::All { all } => all.iter().any(expression_needs_error_or_attempt),
        MatchExpressionV1::Any { any } => any.iter().any(expression_needs_error_or_attempt),
        MatchExpressionV1::Not { not } => expression_needs_error_or_attempt(not),
        MatchExpressionV1::Conditions { conditions } => {
            conditions_need_error_or_attempt(conditions)
        }
    }
}

fn validate_attempt_limit(
    max_attempts: Option<u32>,
    path: &str,
    options: &ValidationOptions,
    errors: &mut Vec<ValidationError>,
) {
    if max_attempts.is_some_and(|limit| limit == 0 || limit > options.max_attempts) {
        errors.push(ValidationError::new(
            ValidationErrorCode::AttemptBudgetExceeded,
            format!("{path}.then.max_attempts"),
            format!(
                "action attempt limit must be between 1 and {}",
                options.max_attempts
            ),
        ));
    }
}

fn require_capability(
    capabilities: &HashSet<Capability>,
    required: Capability,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    if !capabilities.contains(&required) {
        errors.push(ValidationError::new(
            ValidationErrorCode::MissingCapability,
            format!("{path}.capabilities"),
            format!("action or matcher requires capability {required:?}"),
        ));
    }
}

pub(crate) fn parse_status(status: &StatusMatcherSpec) -> Option<(u16, u16)> {
    match status {
        StatusMatcherSpec::Exact(value) if (100..=599).contains(value) => Some((*value, *value)),
        StatusMatcherSpec::Exact(_) => None,
        StatusMatcherSpec::RangeOrClass(value) => {
            let lower = value.to_ascii_lowercase();
            if lower.len() == 3 && lower.ends_with("xx") {
                let class = lower.as_bytes()[0];
                if (b'1'..=b'5').contains(&class) {
                    let start = u16::from(class - b'0') * 100;
                    return Some((start, start + 99));
                }
                return None;
            }
            let (start, end) = lower.split_once('-')?;
            let start: u16 = start.parse().ok()?;
            let end: u16 = end.parse().ok()?;
            ((100..=599).contains(&start) && (100..=599).contains(&end) && start <= end)
                .then_some((start, end))
        }
    }
}

pub(crate) fn starts_like_version_requirement(value: &str) -> bool {
    value.starts_with(['=', '>', '<', '^', '~', '*']) || value.contains(',')
}

pub(crate) fn compile_glob_regex(pattern: &str) -> Result<Regex, String> {
    if pattern.is_empty() {
        return Err("pattern must not be empty".to_owned());
    }
    let mut output = String::from("(?i)^");
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => output.push_str(".*"),
            '?' => output.push('.'),
            '[' => {
                output.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    output.push('^');
                }
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == ']' {
                        output.push(']');
                        closed = true;
                        break;
                    }
                    if inner == '\\' {
                        output.push_str("\\\\");
                    } else {
                        output.push(inner);
                    }
                }
                if !closed {
                    return Err(format!("unclosed character class in glob {pattern:?}"));
                }
            }
            other => output.push_str(&regex::escape(&other.to_string())),
        }
    }
    output.push('$');
    Regex::new(&output).map_err(|error| error.to_string())
}

fn valid_json_pointer(pointer: &str) -> bool {
    if !pointer.starts_with('/') {
        return false;
    }
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            if index + 1 >= bytes.len() || !matches!(bytes[index + 1], b'0' | b'1') {
                return false;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionSpecV1, ProviderModeV1, RerouteActionSpecV1, RouteScopeKindV1};

    fn rule() -> RuleSpecV1 {
        RuleSpecV1 {
            id: "test.rule".into(),
            name: "Test".into(),
            description: None,
            enabled: true,
            priority: 0,
            hook: HookPoint::AttemptResult,
            capabilities: vec![Capability::RouteOverride],
            when: MatchConditionsV1 {
                status: vec![StatusMatcherSpec::RangeOrClass("5xx".into())],
                ..Default::default()
            },
            expression: None,
            action: ActionSpecV1 {
                reroute: Some(RerouteActionSpecV1 {
                    model: Some("gpt-test".into()),
                    equivalent_class: None,
                    providers: vec!["openai".into()],
                    provider_mode: ProviderModeV1::Only,
                    scope: RouteScopeKindV1::Request,
                    ttl_seconds: None,
                    notice: None,
                    reason: "test".into(),
                    max_attempts: Some(3),
                    required_capabilities: Default::default(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn unknown_version_and_duplicate_ids_are_stable_errors() {
        let value = RuleSetV1 {
            api_version: 99,
            rules: vec![rule(), rule()],
        };
        let errors = validate_rule_set(&value, &Default::default(), None);
        assert_eq!(errors[0].code, ValidationErrorCode::UnsupportedApiVersion);
    }

    #[test]
    fn catalog_rejects_model_provider_and_capability_contradictions() {
        let mut rule = rule();
        let reroute = rule.action.reroute.as_mut().unwrap();
        reroute.providers = vec!["anthropic".into()];
        reroute.required_capabilities.vision = true;
        let mut catalog = ValidationCatalog::default();
        catalog.insert("gpt-test", "openai", ModelCapabilities::default());
        let errors = validate_rule_set(
            &RuleSetV1 {
                api_version: 1,
                rules: vec![rule],
            },
            &Default::default(),
            Some(&catalog),
        );
        assert!(errors
            .iter()
            .any(|error| error.code == ValidationErrorCode::ModelProviderContradiction));
        assert!(errors
            .iter()
            .any(|error| error.code == ValidationErrorCode::TargetCapabilityMismatch));
    }
}
