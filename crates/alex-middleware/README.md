# alex-middleware

Runtime-neutral middleware policy types and compiled declarative evaluation for
Alexandria. This crate deliberately does not dispatch HTTP requests, select
accounts, access credentials, or own retry loops.

The proxy integration follows this sequence for a failed attempt:

```rust,ignore
let engine = CompiledRuleSetV1::compile(rule_set)?;

// `head_context` contains status/headers/error metadata and an uninspected body.
let plan = engine.inspection_plan(&head_context);
if plan.needs_body {
    // The proxy reads its configured bounded prefix and fills BodyView once.
}

let result = engine.evaluate_attempt(&attempt_context);
attempt_guard.validate_decision(&attempt_context, &result.decision, committed)?;
// The trusted proxy executes result.decision.
```

Important entry points:

- `RuleSetV1`, `RuleSpecV1`, `MatchConditionsV1`, and `MatchExpressionV1` are
  the JSON/TOML schema.
- `CompiledRuleSetV1` validates, compiles, indexes, and evaluates rules.
- `SafeHeaders::from_untrusted` removes secret and hop-by-hop headers.
- `AttemptGuard` enforces attempt, account, route, and repeated-target budgets.
- `fable_to_sol_rule` and the `alex.*` constructors are ordinary public rules;
  they do not use private matcher or decision paths.

Body decoding and provider error-envelope parsing remain proxy responsibilities.
The proxy supplies a bounded `BodyView` and normalized `ErrorInfo`, allowing this
crate to remain independent of Axum, reqwest, and provider clients.
