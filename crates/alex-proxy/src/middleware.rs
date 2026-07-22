use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use alex_middleware::{
    fable_to_sol_rule, AttemptResultContext, CompiledRuleSetV1, EvaluationControl,
    EvaluationResult, ProviderModeV1, RuleSetV1, RuleSpecV1, ValidationError, ValidationErrorCode,
    ValidationOptions, API_VERSION_V1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{now_ms, ProtectionPolicy, SubstitutionConfig};

const RULES_FILE: &str = "rules.toml";
const LEASES_FILE: &str = "leases.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MiddlewareSettings {
    pub enabled: bool,
    pub error_body_limit_bytes: usize,
    pub max_attempts: u32,
    pub default_script_timeout_ms: u64,
    pub default_script_max_operations: u64,
    pub fail_mode: String,
}

impl Default for MiddlewareSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            error_body_limit_bytes: 64 * 1024,
            max_attempts: 3,
            default_script_timeout_ms: 10,
            default_script_max_operations: 10_000,
            fail_mode: "open".into(),
        }
    }
}

impl MiddlewareSettings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1_024..=1024 * 1024).contains(&self.error_body_limit_bytes) {
            return Err("error_body_limit_bytes must be between 1024 and 1048576".into());
        }
        if !(1..=10).contains(&self.max_attempts) {
            return Err("max_attempts must be between 1 and 10".into());
        }
        if !(1..=1_000).contains(&self.default_script_timeout_ms) {
            return Err("default_script_timeout_ms must be between 1 and 1000".into());
        }
        if !(100..=10_000_000).contains(&self.default_script_max_operations) {
            return Err("default_script_max_operations must be between 100 and 10000000".into());
        }
        if !matches!(self.fail_mode.as_str(), "open" | "closed") {
            return Err("fail_mode must be 'open' or 'closed'".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MiddlewareRouteLease {
    pub id: String,
    pub harness: String,
    pub session_id: String,
    pub original_model: String,
    pub target: alex_middleware::RouteTarget,
    pub source_middleware_id: String,
    pub reason: String,
    pub created_ms: i64,
    pub last_used_ms: i64,
    pub expires_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct StoredMiddleware {
    api_version: u16,
    settings: MiddlewareSettings,
    rules: Vec<RuleSpecV1>,
}

impl Default for StoredMiddleware {
    fn default() -> Self {
        Self {
            api_version: API_VERSION_V1,
            settings: MiddlewareSettings::default(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct RuleStats {
    hit_count: u64,
    last_matched_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct MiddlewareRuntime {
    root: PathBuf,
    pub settings: MiddlewareSettings,
    pub generation: u64,
    pub last_reload_ms: i64,
    rules: Vec<RuleSpecV1>,
    stored_rules: Vec<RuleSpecV1>,
    compiled: CompiledRuleSetV1,
    stats: HashMap<String, RuleStats>,
    errors: Vec<String>,
    substitution: SubstitutionConfig,
    protection: ProtectionPolicy,
}

impl MiddlewareRuntime {
    pub(crate) fn load(root: PathBuf, substitution: SubstitutionConfig) -> Self {
        let protection = ProtectionPolicy::default();
        let stored = read_stored(&root).unwrap_or_default();
        let settings = if stored.settings.validate().is_ok() {
            stored.settings
        } else {
            MiddlewareSettings::default()
        };
        let mut runtime =
            Self::from_parts(root, settings, stored.rules, substitution, protection, 1);
        runtime.last_reload_ms = now_ms();
        runtime
    }

    fn from_parts(
        root: PathBuf,
        settings: MiddlewareSettings,
        stored_rules: Vec<RuleSpecV1>,
        substitution: SubstitutionConfig,
        protection: ProtectionPolicy,
        generation: u64,
    ) -> Self {
        let mut rules = merged_rules(&substitution, &protection, &stored_rules);
        let (compiled, errors) = match compile_rules(&rules, settings.max_attempts, &protection) {
            Ok(compiled) => compiled,
            Err(validation_errors) => {
                // Invalid user configuration must never disable the shipped
                // failover policies. Keep the invalid file for correction,
                // expose stable diagnostics, and run the last safe built-ins.
                rules = merged_rules(&substitution, &protection, &[]);
                let (fallback, _) = compile_rules(&rules, settings.max_attempts, &protection)
                    .expect("shipped middleware rules compile");
                (fallback, validation_messages(&validation_errors))
            }
        };
        Self {
            root,
            settings,
            generation,
            last_reload_ms: now_ms(),
            rules,
            stored_rules,
            compiled,
            stats: HashMap::new(),
            errors,
            substitution,
            protection,
        }
    }

    pub(crate) fn evaluate(
        &self,
        context: &AttemptResultContext,
        no_substitute: bool,
    ) -> EvaluationResult {
        if !self.settings.enabled {
            return EvaluationResult::default();
        }
        self.compiled
            .evaluate_attempt_with(context, EvaluationControl { no_substitute })
    }

    pub(crate) fn inspection_plan(
        &self,
        context: &AttemptResultContext,
    ) -> alex_middleware::BodyInspectionPlan {
        if !self.settings.enabled {
            return Default::default();
        }
        self.compiled.inspection_plan(context)
    }

    pub(crate) fn note_evaluation(&mut self, evaluation: &EvaluationResult) {
        let now = now_ms();
        for record in evaluation
            .records
            .iter()
            .filter(|record| record.state == alex_middleware::MatchState::Matched)
        {
            let stats = self.stats.entry(record.rule_id.clone()).or_default();
            stats.hit_count = stats.hit_count.saturating_add(1);
            stats.last_matched_ms = Some(now);
        }
    }

    pub(crate) fn status_json(&self, leases: Vec<MiddlewareRouteLease>) -> Value {
        let rules = self
            .rules
            .iter()
            .map(|rule| {
                let mut value = serde_json::to_value(rule).unwrap_or_default();
                if let Some(object) = value.as_object_mut() {
                    object.insert("api_version".into(), json!(API_VERSION_V1));
                    object.insert("built_in".into(), json!(is_builtin_id(&rule.id)));
                    let stats = self.stats.get(&rule.id).cloned().unwrap_or_default();
                    object.insert("hit_count".into(), json!(stats.hit_count));
                    object.insert("last_matched_ms".into(), json!(stats.last_matched_ms));
                }
                value
            })
            .collect::<Vec<_>>();
        json!({
            "settings": self.settings,
            "generation": self.generation.to_string(),
            "last_reload_ms": self.last_reload_ms,
            "rules": rules,
            // Declarative middleware ships first. The stable DTO/decision ABI
            // intentionally leaves room for precompiled Rhai in the next beta.
            "scripts": [],
            "leases": leases,
            "errors": self.errors,
            "rhai": {"status": "deferred", "reason": "declarative beta performance and ABI validation first"},
        })
    }

    pub(crate) fn rules(&self) -> &[RuleSpecV1] {
        &self.rules
    }

    pub(crate) fn reroute_effort(&self, rule_id: &str) -> Option<String> {
        self.rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .and_then(|rule| rule.action.reroute.as_ref())
            .and_then(|reroute| reroute.effort.clone())
    }

    pub(crate) fn validate_rule(&self, rule: RuleSpecV1) -> Vec<ValidationError> {
        let set = RuleSetV1 {
            api_version: API_VERSION_V1,
            rules: vec![rule.clone()],
        };
        let mut errors = alex_middleware::validate_rule_set(
            &set,
            &ValidationOptions {
                max_attempts: self.settings.max_attempts,
                ..Default::default()
            },
            None,
        );
        errors.extend(runtime_route_validation_errors(
            std::slice::from_ref(&rule),
            &self.protection,
        ));
        errors
    }

    pub(crate) fn create_rule(&mut self, rule: RuleSpecV1) -> Result<RuleSpecV1, String> {
        if self.rules.iter().any(|existing| existing.id == rule.id) {
            return Err(format!("middleware '{}' already exists", rule.id));
        }
        let errors = self.validate_rule(rule.clone());
        if !errors.is_empty() {
            return Err(validation_messages(&errors).join("; "));
        }
        self.replace_stored_rule(rule.clone(), false)?;
        Ok(rule)
    }

    pub(crate) fn replace_rule(
        &mut self,
        path_id: &str,
        mut rule: RuleSpecV1,
    ) -> Result<RuleSpecV1, String> {
        if path_id != rule.id {
            return Err("path middleware ID must match the rule ID".into());
        }
        if !self.rules.iter().any(|existing| existing.id == path_id) {
            return Err(format!("middleware '{path_id}' not found"));
        }
        // A built-in write is an ordinary persisted override with the same
        // public schema; deleting it later restores the shipped template.
        rule.id = path_id.to_string();
        let errors = self.validate_rule(rule.clone());
        if !errors.is_empty() {
            return Err(validation_messages(&errors).join("; "));
        }
        self.replace_stored_rule(rule.clone(), true)?;
        Ok(rule)
    }

    fn replace_stored_rule(&mut self, rule: RuleSpecV1, replace: bool) -> Result<(), String> {
        let mut stored = self.stored_rules.clone();
        if let Some(index) = stored.iter().position(|existing| existing.id == rule.id) {
            stored[index] = rule;
        } else if replace && is_builtin_id(&rule.id) {
            stored.push(rule);
        } else if replace {
            return Err(format!("middleware '{}' not found", rule.id));
        } else {
            stored.push(rule);
        }
        self.apply_stored(self.settings.clone(), stored)
    }

    pub(crate) fn delete_rule(&mut self, id: &str) -> Result<bool, String> {
        if is_builtin_id(id) {
            return Err("built-in middleware cannot be deleted; disable it instead".into());
        }
        let mut stored = self.stored_rules.clone();
        let old_len = stored.len();
        stored.retain(|rule| rule.id != id);
        if stored.len() == old_len {
            return Ok(false);
        }
        self.apply_stored(self.settings.clone(), stored)?;
        Ok(true)
    }

    pub(crate) fn update_settings(&mut self, settings: MiddlewareSettings) -> Result<(), String> {
        settings.validate()?;
        self.apply_stored(settings, self.stored_rules.clone())
    }

    pub(crate) fn reload(&mut self) -> Result<(), String> {
        let stored = read_stored(&self.root)?;
        stored.settings.validate()?;
        self.apply_stored_without_persist(stored.settings, stored.rules)
    }

    pub(crate) fn set_legacy_protection(&mut self, protection: ProtectionPolicy) {
        let rules = merged_rules(&self.substitution, &protection, &self.stored_rules);
        match compile_rules(&rules, self.settings.max_attempts, &protection) {
            Ok((compiled, _)) => {
                self.protection = protection;
                self.rules = rules;
                self.compiled = compiled;
                self.generation = self.generation.saturating_add(1);
                self.last_reload_ms = now_ms();
                self.errors.clear();
            }
            Err(errors) => self.errors = validation_messages(&errors),
        }
    }

    fn apply_stored(
        &mut self,
        settings: MiddlewareSettings,
        stored_rules: Vec<RuleSpecV1>,
    ) -> Result<(), String> {
        let rules = merged_rules(&self.substitution, &self.protection, &stored_rules);
        let (compiled, _) = compile_rules(&rules, settings.max_attempts, &self.protection)
            .map_err(|errors| validation_messages(&errors).join("; "))?;
        persist_stored(
            &self.root,
            &StoredMiddleware {
                api_version: API_VERSION_V1,
                settings: settings.clone(),
                rules: stored_rules.clone(),
            },
        )?;
        self.settings = settings;
        self.stored_rules = stored_rules;
        self.rules = rules;
        self.compiled = compiled;
        self.generation = self.generation.saturating_add(1);
        self.last_reload_ms = now_ms();
        self.errors.clear();
        Ok(())
    }

    fn apply_stored_without_persist(
        &mut self,
        settings: MiddlewareSettings,
        stored_rules: Vec<RuleSpecV1>,
    ) -> Result<(), String> {
        let rules = merged_rules(&self.substitution, &self.protection, &stored_rules);
        let (compiled, _) = compile_rules(&rules, settings.max_attempts, &self.protection)
            .map_err(|errors| validation_messages(&errors).join("; "))?;
        self.settings = settings;
        self.stored_rules = stored_rules;
        self.rules = rules;
        self.compiled = compiled;
        self.generation = self.generation.saturating_add(1);
        self.last_reload_ms = now_ms();
        self.errors.clear();
        Ok(())
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn equivalent_targets(&self, class: &str) -> Vec<(String, String)> {
        equivalent_targets_for_policy(&self.protection, class)
    }

    pub(crate) fn equivalence_classes_for_model(&self, model: &str) -> Vec<String> {
        let canonical = crate::canonical_model_alias(model);
        self.protection
            .equivalencies
            .iter()
            .filter(|(class, targets)| {
                crate::canonical_model_alias(class) == canonical
                    || targets
                        .values()
                        .any(|target| crate::canonical_model_alias(target) == canonical)
            })
            .map(|(class, _)| class.clone())
            .collect()
    }
}

fn compile_rules(
    rules: &[RuleSpecV1],
    max_attempts: u32,
    protection: &ProtectionPolicy,
) -> Result<(CompiledRuleSetV1, Vec<String>), Vec<ValidationError>> {
    let runtime_errors = runtime_route_validation_errors(rules, protection);
    if !runtime_errors.is_empty() {
        return Err(runtime_errors);
    }
    CompiledRuleSetV1::compile_with(
        RuleSetV1 {
            api_version: API_VERSION_V1,
            rules: rules.to_vec(),
        },
        &ValidationOptions {
            max_attempts,
            ..Default::default()
        },
        None,
    )
    .map(|compiled| (compiled, Vec::new()))
    .map_err(|error| error.errors)
}

fn equivalent_targets_for_policy(
    protection: &ProtectionPolicy,
    class: &str,
) -> Vec<(String, String)> {
    let canonical = crate::canonical_model_alias(class);
    protection
        .equivalencies
        .iter()
        .find(|(configured, _)| crate::canonical_model_alias(configured) == canonical)
        .map(|(_, targets)| {
            targets
                .iter()
                .map(|(provider, model)| (provider.clone(), model.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn runtime_route_validation_errors(
    rules: &[RuleSpecV1],
    protection: &ProtectionPolicy,
) -> Vec<ValidationError> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.enabled)
        .filter_map(|(index, rule)| {
            let reroute = rule.action.reroute.as_ref()?;
            let class = reroute.equivalent_class.as_deref()?;
            let targets = equivalent_targets_for_policy(protection, class);
            if targets.is_empty() {
                return Some(ValidationError {
                    code: ValidationErrorCode::UnknownTargetModel,
                    path: format!("rules[{index}].then.reroute.equivalent_class"),
                    message: format!(
                        "equivalence class {class:?} has no configured provider/model targets"
                    ),
                });
            }
            let provider_allowed = match reroute.provider_mode {
                ProviderModeV1::Any | ProviderModeV1::Prefer => true,
                ProviderModeV1::Only => targets.iter().any(|(provider, _)| {
                    reroute
                        .providers
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(provider))
                }),
                ProviderModeV1::Exclude => targets.iter().any(|(provider, _)| {
                    !reroute
                        .providers
                        .iter()
                        .any(|excluded| excluded.eq_ignore_ascii_case(provider))
                }),
            };
            (!provider_allowed).then(|| ValidationError {
                code: ValidationErrorCode::InvalidProviderConstraint,
                path: format!("rules[{index}].then.reroute.providers"),
                message: format!(
                    "provider constraint excludes every target in equivalence class {class:?}"
                ),
            })
        })
        .collect()
}

fn validation_messages(errors: &[ValidationError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("{}: {}", error.path, error.message))
        .collect()
}

fn is_builtin_id(id: &str) -> bool {
    id == alex_middleware::FABLE_TO_SOL_ID
}

fn is_retired_builtin_id(id: &str) -> bool {
    id == "alex.account-failover"
        || id == "alex.auth-failover"
        || id == "alex.model-equivalence-failover"
        || id.starts_with("alex.model-equivalence-failover.")
        || id == "alex.model-fallbacks"
        || id.starts_with("alex.model-fallbacks.")
        || id == "example.fable-overload-to-sol"
}

fn merge_by_id(defaults: Vec<RuleSpecV1>, overrides: &[RuleSpecV1]) -> Vec<RuleSpecV1> {
    let mut rules = BTreeMap::<String, RuleSpecV1>::new();
    for rule in defaults {
        rules.insert(rule.id.clone(), rule);
    }
    for rule in overrides {
        if !is_retired_builtin_id(&rule.id) {
            rules.insert(rule.id.clone(), rule.clone());
        }
    }
    rules.into_values().collect()
}

fn merged_rules(
    _substitution: &SubstitutionConfig,
    _protection: &ProtectionPolicy,
    stored: &[RuleSpecV1],
) -> Vec<RuleSpecV1> {
    merge_by_id(vec![fable_to_sol_rule()], stored)
}

fn read_stored(root: &Path) -> Result<StoredMiddleware, String> {
    let path = root.join(RULES_FILE);
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(StoredMiddleware::default())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn persist_stored(root: &Path, stored: &StoredMiddleware) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let bytes = toml::to_string_pretty(stored)
        .map_err(|error| error.to_string())?
        .into_bytes();
    atomic_write(&root.join(RULES_FILE), &bytes)
}

pub(crate) fn load_leases(root: &Path) -> Vec<MiddlewareRouteLease> {
    fs::read(root.join(LEASES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn persist_leases(root: &Path, leases: &[MiddlewareRouteLease]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(leases).map_err(|error| error.to_string())?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    atomic_write(&root.join(LEASES_FILE), &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::File::create(&tmp).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&tmp, path).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "alex-middleware-runtime-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn stored_rule_replaces_builtin_and_survives_reload() {
        let root = temp_root("builtin-override");
        let mut runtime = MiddlewareRuntime::load(root.clone(), SubstitutionConfig::default());
        let mut fallback = runtime
            .rules()
            .iter()
            .find(|rule| rule.id == alex_middleware::FABLE_TO_SOL_ID)
            .unwrap()
            .clone();
        fallback.enabled = false;
        runtime
            .replace_rule(&fallback.id.clone(), fallback)
            .unwrap();

        let reloaded = MiddlewareRuntime::load(root, SubstitutionConfig::default());
        assert!(
            !reloaded
                .rules()
                .iter()
                .find(|rule| rule.id == alex_middleware::FABLE_TO_SOL_ID)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn legacy_fallbacks_do_not_create_additional_middleware() {
        let runtime = MiddlewareRuntime::load(
            temp_root("legacy-fallback"),
            SubstitutionConfig {
                enabled: true,
                fallbacks: BTreeMap::from([(
                    "claude-fable-5".into(),
                    vec!["openai/gpt-5.6-sol".into()],
                )]),
            },
        );
        assert_eq!(runtime.rules().len(), 1);
        assert_eq!(runtime.rules()[0].id, alex_middleware::FABLE_TO_SOL_ID);
    }

    #[test]
    fn equivalence_catalog_validates_targets_and_indexes_members() {
        let mut runtime = MiddlewareRuntime::load(
            temp_root("equivalence-catalog"),
            SubstitutionConfig::default(),
        );
        let mut rule = fable_to_sol_rule();
        rule.id = "custom.equivalent-fallback".into();
        let reroute = rule.action.reroute.as_mut().unwrap();
        reroute.model = None;
        reroute.equivalent_class = Some("claude-fable-5".into());

        let errors = runtime.validate_rule(rule.clone());
        assert!(errors
            .iter()
            .any(|error| error.code == ValidationErrorCode::UnknownTargetModel));
        assert!(runtime.create_rule(rule.clone()).is_err());

        runtime.set_legacy_protection(ProtectionPolicy {
            enabled: true,
            equivalencies: BTreeMap::from([(
                "claude-fable-5".into(),
                BTreeMap::from([("openai".into(), "gpt-5.6-sol".into())]),
            )]),
            ..Default::default()
        });
        assert!(runtime.validate_rule(rule.clone()).is_empty());
        assert_eq!(
            runtime.equivalent_targets("fable-5"),
            vec![("openai".into(), "gpt-5.6-sol".into())]
        );
        assert_eq!(
            runtime.equivalence_classes_for_model("gpt-5.6-sol"),
            vec!["claude-fable-5"]
        );
        runtime.create_rule(rule).unwrap();
    }

    #[test]
    fn lease_persistence_round_trips() {
        let root = temp_root("leases");
        let lease = MiddlewareRouteLease {
            id: "lease-1".into(),
            harness: "claude".into(),
            session_id: "session-1".into(),
            original_model: "claude-fable-5".into(),
            target: alex_middleware::RouteTarget::Exact {
                model: "gpt-5.6-sol".into(),
                providers: alex_middleware::ProviderConstraint::Only(vec!["openai".into()]),
            },
            source_middleware_id: alex_middleware::FABLE_TO_SOL_ID.into(),
            reason: "test".into(),
            created_ms: 1,
            last_used_ms: 1,
            expires_ms: 2,
        };
        persist_leases(&root, std::slice::from_ref(&lease)).unwrap();
        assert_eq!(load_leases(&root)[0].id, lease.id);
    }
}
