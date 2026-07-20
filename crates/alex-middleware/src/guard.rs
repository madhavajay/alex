use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{AttemptDecision, AttemptResultContext, RouteScope, RouteTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptBudget {
    pub max_attempts: u32,
    pub max_distinct_routes: u32,
    pub max_accounts_per_route: u32,
}

impl Default for AttemptBudget {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_distinct_routes: 3,
            max_accounts_per_route: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttemptTarget {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub account_id: Option<String>,
}

impl AttemptTarget {
    fn route_key(&self) -> (String, String) {
        (
            self.provider.to_ascii_lowercase(),
            self.model.to_ascii_lowercase(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardErrorCode {
    AttemptBudgetExhausted,
    RouteBudgetExhausted,
    AccountBudgetExhausted,
    RepeatedTarget,
    DownstreamCommitted,
    UnstableSession,
    NonPortableSession,
    RepeatedRouteTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardError {
    pub code: GuardErrorCode,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct AttemptGuard {
    budget: AttemptBudget,
    attempts: u32,
    attempted_targets: HashSet<AttemptTarget>,
    attempted_routes: HashSet<(String, String)>,
    proposed_route_targets: HashSet<String>,
}

impl AttemptGuard {
    pub fn new(budget: AttemptBudget) -> Self {
        Self {
            budget,
            attempts: 0,
            attempted_targets: HashSet::new(),
            attempted_routes: HashSet::new(),
            proposed_route_targets: HashSet::new(),
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn attempted_targets(&self) -> impl Iterator<Item = &AttemptTarget> {
        self.attempted_targets.iter()
    }

    /// Records an actual dispatch target. Call this after account and route
    /// resolution but before sending upstream.
    pub fn record_attempt(&mut self, target: AttemptTarget) -> Result<(), GuardError> {
        if self.attempts >= self.budget.max_attempts {
            return Err(guard_error(
                GuardErrorCode::AttemptBudgetExhausted,
                "global upstream attempt budget exhausted",
            ));
        }
        if self.attempted_targets.contains(&target) {
            return Err(guard_error(
                GuardErrorCode::RepeatedTarget,
                format!(
                    "provider/model/account target was already attempted: {}/{}/{}",
                    target.provider,
                    target.model,
                    target.account_id.as_deref().unwrap_or("<none>")
                ),
            ));
        }
        let route = target.route_key();
        if !self.attempted_routes.contains(&route)
            && self.attempted_routes.len() >= self.budget.max_distinct_routes as usize
        {
            return Err(guard_error(
                GuardErrorCode::RouteBudgetExhausted,
                "distinct model/provider route budget exhausted",
            ));
        }
        let accounts_on_route = self
            .attempted_targets
            .iter()
            .filter(|attempted| attempted.route_key() == route)
            .count();
        if accounts_on_route >= self.budget.max_accounts_per_route as usize {
            return Err(guard_error(
                GuardErrorCode::AccountBudgetExhausted,
                "same-route account attempt budget exhausted",
            ));
        }
        self.attempts += 1;
        self.attempted_routes.insert(route);
        self.attempted_targets.insert(target);
        Ok(())
    }

    /// Validates a middleware decision before route/account resolution. Actual
    /// repeated `(provider, model, account)` protection remains in `record_attempt`.
    pub fn validate_decision(
        &mut self,
        context: &AttemptResultContext,
        decision: &AttemptDecision,
        downstream_committed: bool,
    ) -> Result<(), GuardError> {
        if downstream_committed && decision.is_terminal() {
            return Err(guard_error(
                GuardErrorCode::DownstreamCommitted,
                "middleware cannot return, retry, or reroute after downstream commitment",
            ));
        }
        if self.attempts >= self.budget.max_attempts
            && matches!(
                decision,
                AttemptDecision::RetrySameRoute { .. } | AttemptDecision::Reroute { .. }
            )
        {
            return Err(guard_error(
                GuardErrorCode::AttemptBudgetExhausted,
                "middleware decision exceeds the global attempt budget",
            ));
        }
        if let AttemptDecision::Reroute { target, scope, .. } = decision {
            if let RouteScope::Session { .. } = scope {
                if !context.session.has_stable_id() {
                    return Err(guard_error(
                        GuardErrorCode::UnstableSession,
                        "session reroute requires a stable session ID",
                    ));
                }
                if !context.route.requested.capabilities.portable_history {
                    return Err(guard_error(
                        GuardErrorCode::NonPortableSession,
                        "session reroute requires portable conversation history",
                    ));
                }
            }
            let key = target.cycle_key();
            if !self.proposed_route_targets.insert(key) {
                return Err(guard_error(
                    GuardErrorCode::RepeatedRouteTarget,
                    "middleware proposed a route target already used in this request",
                ));
            }
            if let RouteTarget::Exact { model, providers } = target {
                let selected_provider = &context.route.provider.id;
                let can_be_current_provider = match providers {
                    crate::ProviderConstraint::Any => true,
                    crate::ProviderConstraint::Only(providers)
                    | crate::ProviderConstraint::Prefer(providers) => providers
                        .iter()
                        .any(|provider| provider.eq_ignore_ascii_case(selected_provider)),
                    crate::ProviderConstraint::Exclude(providers) => !providers
                        .iter()
                        .any(|provider| provider.eq_ignore_ascii_case(selected_provider)),
                };
                if model.eq_ignore_ascii_case(&context.route.selected.id)
                    && can_be_current_provider
                    && !context.route.same_route_accounts_remaining
                {
                    return Err(guard_error(
                        GuardErrorCode::RepeatedRouteTarget,
                        "reroute resolves to the exhausted current route",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn guard_error(code: GuardErrorCode, message: impl Into<String>) -> GuardError {
    GuardError {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_enforce_global_account_and_cycle_budgets() {
        let mut guard = AttemptGuard::new(AttemptBudget {
            max_attempts: 3,
            max_distinct_routes: 2,
            max_accounts_per_route: 2,
        });
        let target = |account: &str| AttemptTarget {
            provider: "anthropic".into(),
            model: "fable".into(),
            account_id: Some(account.into()),
        };
        guard.record_attempt(target("a")).unwrap();
        assert_eq!(
            guard.record_attempt(target("a")).unwrap_err().code,
            GuardErrorCode::RepeatedTarget
        );
        guard.record_attempt(target("b")).unwrap();
        assert_eq!(
            guard.record_attempt(target("c")).unwrap_err().code,
            GuardErrorCode::AccountBudgetExhausted
        );
    }
}
