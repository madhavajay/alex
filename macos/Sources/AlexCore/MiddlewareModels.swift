import Foundation

// MARK: - Middleware admin API

/// The runtime-neutral declarative rule exchanged with `/admin/middleware`.
///
/// Optional runtime metadata is decoded when the daemon includes it in the
/// status response. It is omitted by callers creating a rule, keeping this
/// value usable as the canonical `RuleSpecV1` write payload as well.
public struct MiddlewareRuleSpecV1: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var name: String
    public var description: String?
    public var enabled: Bool
    public var priority: Int
    public var hook: MiddlewareHookPoint
    public var capabilities: [String]
    public var when: MiddlewareMatchSpec
    public var expression: MiddlewareMatchExpression?
    public var then: MiddlewareActionSpec

    // Read-only metadata returned by the status endpoint.
    public var builtIn: Bool?
    public var hitCount: Int?
    public var lastMatchedMs: Int64?
    public var validationErrors: [String]?

    public init(
        id: String,
        name: String,
        description: String? = nil,
        enabled: Bool = true,
        priority: Int = 100,
        hook: MiddlewareHookPoint,
        capabilities: [String] = [],
        when: MiddlewareMatchSpec,
        expression: MiddlewareMatchExpression? = nil,
        then: MiddlewareActionSpec,
        builtIn: Bool? = nil,
        hitCount: Int? = nil,
        lastMatchedMs: Int64? = nil,
        validationErrors: [String]? = nil
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.enabled = enabled
        self.priority = priority
        self.hook = hook
        self.capabilities = capabilities
        self.when = when
        self.expression = expression
        self.then = then
        self.builtIn = builtIn
        self.hitCount = hitCount
        self.lastMatchedMs = lastMatchedMs
        self.validationErrors = validationErrors
    }

    public var isBuiltIn: Bool { builtIn == true || id.hasPrefix("alex.") }

    enum CodingKeys: String, CodingKey {
        case id, name, description, enabled, priority, hook, capabilities, when, expression, then
        case builtIn = "built_in"
        case hitCount = "hit_count"
        case lastMatchedMs = "last_matched_ms"
        case validationErrors = "validation_errors"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        name = try values.decodeIfPresent(String.self, forKey: .name) ?? id
        description = try values.decodeIfPresent(String.self, forKey: .description)
        enabled = try values.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        priority = try values.decodeIfPresent(Int.self, forKey: .priority) ?? 100
        hook = try values.decodeIfPresent(MiddlewareHookPoint.self, forKey: .hook) ?? .attemptResult
        capabilities = try values.decodeIfPresent([String].self, forKey: .capabilities) ?? []
        when = try values.decodeIfPresent(MiddlewareMatchSpec.self, forKey: .when) ?? .init()
        expression = try values.decodeIfPresent(MiddlewareMatchExpression.self, forKey: .expression)
        then = try values.decode(MiddlewareActionSpec.self, forKey: .then)
        builtIn = try values.decodeIfPresent(Bool.self, forKey: .builtIn)
        hitCount = try values.decodeIfPresent(Int.self, forKey: .hitCount)
        lastMatchedMs = try values.decodeIfPresent(Int64.self, forKey: .lastMatchedMs)
        validationErrors = try values.decodeIfPresent([String].self, forKey: .validationErrors)
    }
}

public enum MiddlewareHookPoint: String, Codable, Sendable, CaseIterable {
    case requestReceived = "request_received"
    case routePlanned = "route_planned"
    case attemptResult = "attempt_result"
    case responseReady = "response_ready"
    case traceFinalized = "trace_finalized"
}

public enum MiddlewareConditionMode: String, Codable, Sendable, CaseIterable {
    case all
    case any
}

public enum MiddlewareStatusMatcher: Codable, Sendable, Equatable, Hashable {
    case exact(Int)
    case pattern(String)

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if let status = try? value.decode(Int.self) {
            self = .exact(status)
        } else {
            self = .pattern(try value.decode(String.self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var value = encoder.singleValueContainer()
        switch self {
        case let .exact(status): try value.encode(status)
        case let .pattern(pattern): try value.encode(pattern)
        }
    }

    public var displayValue: String {
        switch self {
        case let .exact(status): "\(status)"
        case let .pattern(pattern): pattern
        }
    }
}

public struct MiddlewareMatchSpec: Codable, Sendable, Equatable {
    public var harnessNames: [String]?
    public var harnessVersions: [String]?
    public var models: [String]?
    public var efforts: [String]?
    public var providers: [String]?
    public var status: [MiddlewareStatusMatcher]?
    public var errorClasses: [String]?
    public var errorKinds: [String]?
    public var bodyContainsAny: [String]?
    public var stableSession: Bool?

    public init(
        harnessNames: [String]? = nil,
        harnessVersions: [String]? = nil,
        models: [String]? = nil,
        efforts: [String]? = nil,
        providers: [String]? = nil,
        status: [MiddlewareStatusMatcher]? = nil,
        errorClasses: [String]? = nil,
        errorKinds: [String]? = nil,
        bodyContainsAny: [String]? = nil,
        stableSession: Bool? = nil
    ) {
        self.harnessNames = harnessNames
        self.harnessVersions = harnessVersions
        self.models = models
        self.efforts = efforts
        self.providers = providers
        self.status = status
        self.errorClasses = errorClasses
        self.errorKinds = errorKinds
        self.bodyContainsAny = bodyContainsAny
        self.stableSession = stableSession
    }

    enum CodingKeys: String, CodingKey {
        case models, efforts, providers, status
        case harnessNames = "harness_names"
        case harnessVersions = "harness_versions"
        case errorClasses = "error_classes"
        case errorKinds = "error_kinds"
        case bodyContainsAny = "body_contains_any"
        case stableSession = "stable_session"
    }

    public var isEmpty: Bool {
        let lists = [harnessNames, models, efforts, providers, errorClasses, errorKinds, bodyContainsAny]
        return lists.allSatisfy { $0?.isEmpty != false }
            && status?.isEmpty != false
            && harnessVersions?.isEmpty != false
            && stableSession == nil
    }
}

/// The nested `all` / `any` / `not` representation used by RuleSpecV1.
public indirect enum MiddlewareMatchExpression: Codable, Sendable, Equatable {
    case all([MiddlewareMatchExpression])
    case any([MiddlewareMatchExpression])
    case not(MiddlewareMatchExpression)
    case conditions(MiddlewareMatchSpec)

    private enum CodingKeys: String, CodingKey { case all, any, not, conditions }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        if values.contains(.all) {
            self = .all(try values.decode([Self].self, forKey: .all))
        } else if values.contains(.any) {
            self = .any(try values.decode([Self].self, forKey: .any))
        } else if values.contains(.not) {
            self = .not(try values.decode(Self.self, forKey: .not))
        } else {
            self = .conditions(try values.decode(MiddlewareMatchSpec.self, forKey: .conditions))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .all(children): try values.encode(children, forKey: .all)
        case let .any(children): try values.encode(children, forKey: .any)
        case let .not(child): try values.encode(child, forKey: .not)
        case let .conditions(match): try values.encode(match, forKey: .conditions)
        }
    }
}

public struct MiddlewareActionSpec: Codable, Sendable, Equatable {
    public var retrySameRoute: MiddlewareRetryAction?
    public var reroute: MiddlewareRerouteAction?

    public init(
        retrySameRoute: MiddlewareRetryAction? = nil,
        reroute: MiddlewareRerouteAction? = nil
    ) {
        self.retrySameRoute = retrySameRoute
        self.reroute = reroute
    }

    enum CodingKeys: String, CodingKey {
        case retrySameRoute = "retry_same_route"
        case reroute
    }
}

public struct MiddlewareRetryAction: Codable, Sendable, Equatable {
    public var excludeCurrentAccount: Bool
    public var reason: String
    public var maxAttempts: Int?

    public init(
        excludeCurrentAccount: Bool = true,
        reason: String = "Matched middleware rule",
        maxAttempts: Int? = nil
    ) {
        self.excludeCurrentAccount = excludeCurrentAccount
        self.reason = reason
        self.maxAttempts = maxAttempts
    }

    enum CodingKeys: String, CodingKey {
        case reason
        case excludeCurrentAccount = "exclude_current_account"
        case maxAttempts = "max_attempts"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        excludeCurrentAccount = try values.decodeIfPresent(Bool.self, forKey: .excludeCurrentAccount) ?? true
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? "Matched middleware rule"
        maxAttempts = try values.decodeIfPresent(Int.self, forKey: .maxAttempts)
    }
}

public enum MiddlewareProviderMode: String, Codable, Sendable, CaseIterable {
    case any
    case prefer
    case only
    case exclude
}

public enum MiddlewareRouteScope: String, Codable, Sendable, CaseIterable {
    case request
    case session
}

public struct MiddlewareRerouteAction: Codable, Sendable, Equatable {
    public var model: String?
    public var equivalenceClass: String?
    public var providerMode: MiddlewareProviderMode
    public var providers: [String]
    public var scope: MiddlewareRouteScope
    public var ttlSeconds: Int?
    public var notice: String?
    public var effort: String?
    public var reason: String
    public var maxAttempts: Int?
    public var requiredCapabilities: MiddlewareModelCapabilityRequirements

    public init(
        model: String? = nil,
        equivalenceClass: String? = nil,
        providerMode: MiddlewareProviderMode = .any,
        providers: [String] = [],
        scope: MiddlewareRouteScope = .request,
        ttlSeconds: Int? = nil,
        notice: String? = nil,
        effort: String? = nil,
        reason: String = "Matched middleware rule",
        maxAttempts: Int? = nil,
        requiredCapabilities: MiddlewareModelCapabilityRequirements = .init()
    ) {
        self.model = model
        self.equivalenceClass = equivalenceClass
        self.providerMode = providerMode
        self.providers = providers
        self.scope = scope
        self.ttlSeconds = ttlSeconds
        self.notice = notice
        self.effort = effort
        self.reason = reason
        self.maxAttempts = maxAttempts
        self.requiredCapabilities = requiredCapabilities
    }

    enum CodingKeys: String, CodingKey {
        case model, providers, scope, notice, effort, reason
        case equivalenceClass = "equivalent_class"
        case providerMode = "provider_mode"
        case ttlSeconds = "ttl_seconds"
        case maxAttempts = "max_attempts"
        case requiredCapabilities = "required_capabilities"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        model = try values.decodeIfPresent(String.self, forKey: .model)
        equivalenceClass = try values.decodeIfPresent(String.self, forKey: .equivalenceClass)
        providerMode = try values.decodeIfPresent(MiddlewareProviderMode.self, forKey: .providerMode) ?? .any
        providers = try values.decodeIfPresent([String].self, forKey: .providers) ?? []
        scope = try values.decodeIfPresent(MiddlewareRouteScope.self, forKey: .scope) ?? .request
        ttlSeconds = try values.decodeIfPresent(Int.self, forKey: .ttlSeconds)
        notice = try values.decodeIfPresent(String.self, forKey: .notice)
        effort = try values.decodeIfPresent(String.self, forKey: .effort)
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? "Matched middleware rule"
        maxAttempts = try values.decodeIfPresent(Int.self, forKey: .maxAttempts)
        requiredCapabilities = try values.decodeIfPresent(
            MiddlewareModelCapabilityRequirements.self, forKey: .requiredCapabilities) ?? .init()
    }
}

public struct MiddlewareModelCapabilityRequirements: Codable, Sendable, Equatable {
    public var tools: Bool
    public var vision: Bool
    public var reasoning: Bool
    public var portableHistory: Bool

    public init(
        tools: Bool = false,
        vision: Bool = false,
        reasoning: Bool = false,
        portableHistory: Bool = false
    ) {
        self.tools = tools
        self.vision = vision
        self.reasoning = reasoning
        self.portableHistory = portableHistory
    }

    enum CodingKeys: String, CodingKey {
        case tools, vision, reasoning
        case portableHistory = "portable_history"
    }
}

public struct MiddlewareSettings: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var errorBodyLimitBytes: Int
    public var maxAttempts: Int
    public var defaultScriptTimeoutMs: Int
    public var defaultScriptMaxOperations: Int
    public var failMode: String

    public init(
        enabled: Bool = true,
        errorBodyLimitBytes: Int = 65_536,
        maxAttempts: Int = 3,
        defaultScriptTimeoutMs: Int = 10,
        defaultScriptMaxOperations: Int = 10_000,
        failMode: String = "open"
    ) {
        self.enabled = enabled
        self.errorBodyLimitBytes = errorBodyLimitBytes
        self.maxAttempts = maxAttempts
        self.defaultScriptTimeoutMs = defaultScriptTimeoutMs
        self.defaultScriptMaxOperations = defaultScriptMaxOperations
        self.failMode = failMode
    }

    enum CodingKeys: String, CodingKey {
        case enabled
        case errorBodyLimitBytes = "error_body_limit_bytes"
        case maxAttempts = "max_attempts"
        case defaultScriptTimeoutMs = "default_script_timeout_ms"
        case defaultScriptMaxOperations = "default_script_max_operations"
        case failMode = "fail_mode"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try values.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        errorBodyLimitBytes = try values.decodeIfPresent(Int.self, forKey: .errorBodyLimitBytes) ?? 65_536
        maxAttempts = try values.decodeIfPresent(Int.self, forKey: .maxAttempts) ?? 3
        defaultScriptTimeoutMs = try values.decodeIfPresent(Int.self, forKey: .defaultScriptTimeoutMs) ?? 10
        defaultScriptMaxOperations = try values.decodeIfPresent(Int.self, forKey: .defaultScriptMaxOperations) ?? 10_000
        failMode = try values.decodeIfPresent(String.self, forKey: .failMode) ?? "open"
    }
}

public struct MiddlewareScriptStatus: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var script: String
    public var manifest: String?
    public var status: String
    public var hooks: [MiddlewareHookPoint]
    public var capabilities: [String]
    public var error: String?

    public init(
        id: String,
        script: String,
        manifest: String? = nil,
        status: String = "loaded",
        hooks: [MiddlewareHookPoint] = [],
        capabilities: [String] = [],
        error: String? = nil
    ) {
        self.id = id
        self.script = script
        self.manifest = manifest
        self.status = status
        self.hooks = hooks
        self.capabilities = capabilities
        self.error = error
    }

    enum CodingKeys: String, CodingKey {
        case id, script, manifest, status, hooks, capabilities, error
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        script = try values.decodeIfPresent(String.self, forKey: .script) ?? id
        manifest = try values.decodeIfPresent(String.self, forKey: .manifest)
        status = try values.decodeIfPresent(String.self, forKey: .status) ?? "loaded"
        hooks = try values.decodeIfPresent([MiddlewareHookPoint].self, forKey: .hooks) ?? []
        capabilities = try values.decodeIfPresent([String].self, forKey: .capabilities) ?? []
        error = try values.decodeIfPresent(String.self, forKey: .error)
    }
}

public struct MiddlewareRouteLease: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var harness: String?
    public var sessionId: String
    public var originalModel: String
    public var target: MiddlewareLeaseRouteTarget
    public var sourceMiddlewareId: String
    public var reason: String?
    public var createdMs: Int64?
    public var lastUsedMs: Int64?
    public var expiresMs: Int64

    public init(
        id: String,
        harness: String? = nil,
        sessionId: String,
        originalModel: String,
        target: MiddlewareLeaseRouteTarget,
        sourceMiddlewareId: String,
        reason: String? = nil,
        createdMs: Int64? = nil,
        lastUsedMs: Int64? = nil,
        expiresMs: Int64
    ) {
        self.id = id
        self.harness = harness
        self.sessionId = sessionId
        self.originalModel = originalModel
        self.target = target
        self.sourceMiddlewareId = sourceMiddlewareId
        self.reason = reason
        self.createdMs = createdMs
        self.lastUsedMs = lastUsedMs
        self.expiresMs = expiresMs
    }

    enum CodingKeys: String, CodingKey {
        case id, harness, target, reason
        case sessionId = "session_id"
        case originalModel = "original_model"
        case sourceMiddlewareId = "source_middleware_id"
        case createdMs = "created_ms"
        case lastUsedMs = "last_used_ms"
        case expiresMs = "expires_ms"
    }
}

public struct MiddlewareLeaseRouteTarget: Codable, Sendable, Equatable {
    public var kind: String
    public var model: String?
    public var equivalenceClass: String?
    public var providers: MiddlewareLeaseProviderConstraint

    public init(
        kind: String,
        model: String? = nil,
        equivalenceClass: String? = nil,
        providers: MiddlewareLeaseProviderConstraint = .any
    ) {
        self.kind = kind
        self.model = model
        self.equivalenceClass = equivalenceClass
        self.providers = providers
    }

    enum CodingKeys: String, CodingKey {
        case kind, model, providers
        case equivalenceClass = "class"
    }

    public var displayModel: String {
        model ?? equivalenceClass.map { "Equivalent: \($0)" } ?? "Unknown target"
    }

    public var displayProviders: String {
        switch providers {
        case .any: "any provider"
        case let .only(values), let .prefer(values), let .exclude(values): values.joined(separator: ", ")
        }
    }
}

/// Serde's externally-tagged provider constraint used by route leases.
public enum MiddlewareLeaseProviderConstraint: Codable, Sendable, Equatable {
    case any
    case only([String])
    case prefer([String])
    case exclude([String])

    private enum CodingKeys: String, CodingKey { case only, prefer, exclude }

    public init(from decoder: Decoder) throws {
        if let value = try? decoder.singleValueContainer().decode(String.self), value == "any" {
            self = .any
            return
        }
        let values = try decoder.container(keyedBy: CodingKeys.self)
        if values.contains(.only) {
            self = .only(try values.decode([String].self, forKey: .only))
        } else if values.contains(.prefer) {
            self = .prefer(try values.decode([String].self, forKey: .prefer))
        } else {
            self = .exclude(try values.decode([String].self, forKey: .exclude))
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case .any:
            var value = encoder.singleValueContainer()
            try value.encode("any")
        case let .only(providers):
            var values = encoder.container(keyedBy: CodingKeys.self)
            try values.encode(providers, forKey: .only)
        case let .prefer(providers):
            var values = encoder.container(keyedBy: CodingKeys.self)
            try values.encode(providers, forKey: .prefer)
        case let .exclude(providers):
            var values = encoder.container(keyedBy: CodingKeys.self)
            try values.encode(providers, forKey: .exclude)
        }
    }
}

public struct MiddlewareLeasesResponse: Codable, Sendable, Equatable {
    public var leases: [MiddlewareRouteLease]

    public init(leases: [MiddlewareRouteLease]) {
        self.leases = leases
    }
}

public struct MiddlewareActivityResponse: Codable, Sendable, Equatable {
    public var events: [MiddlewareActivityEvent]

    public init(events: [MiddlewareActivityEvent] = []) {
        self.events = events
    }
}

public struct MiddlewareActivityEvent: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var tsMs: Int64?
    public var sessionId: String?
    public var harness: String?
    public var requestedModel: String?
    public var routedModel: String?
    public var servedModel: String?
    public var status: Int?
    public var substituted: Bool
    public var substitutionReason: String?
    public var attempts: [MiddlewareActivityAttempt]

    enum CodingKeys: String, CodingKey {
        case id, harness, status, substituted, attempts
        case tsMs = "ts_ms"
        case sessionId = "session_id"
        case requestedModel = "requested_model"
        case routedModel = "routed_model"
        case servedModel = "served_model"
        case substitutionReason = "substitution_reason"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        tsMs = try values.decodeIfPresent(Int64.self, forKey: .tsMs)
        sessionId = try values.decodeIfPresent(String.self, forKey: .sessionId)
        harness = try values.decodeIfPresent(String.self, forKey: .harness)
        requestedModel = try values.decodeIfPresent(String.self, forKey: .requestedModel)
        routedModel = try values.decodeIfPresent(String.self, forKey: .routedModel)
        servedModel = try values.decodeIfPresent(String.self, forKey: .servedModel)
        status = try values.decodeIfPresent(Int.self, forKey: .status)
        substituted = try values.decodeIfPresent(Bool.self, forKey: .substituted) ?? false
        substitutionReason = try values.decodeIfPresent(String.self, forKey: .substitutionReason)
        attempts = try values.decodeIfPresent([MiddlewareActivityAttempt].self, forKey: .attempts) ?? []
    }

    public var matchedDecisions: [MiddlewareActivityDecision] {
        attempts.flatMap(\.middlewareDecisions).filter { $0.state == "matched" }
    }

    public var finalModel: String? { servedModel ?? routedModel }
}

public struct MiddlewareActivityAttempt: Codable, Sendable, Equatable {
    public var provider: String?
    public var model: String?
    public var status: Int?
    public var errorKind: String?
    public var errorCode: String?
    public var middlewareDecisions: [MiddlewareActivityDecision]

    enum CodingKeys: String, CodingKey {
        case provider, model, status
        case errorKind = "error_kind"
        case errorCode = "error_code"
        case middlewareDecisions = "middleware_decisions"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        provider = try values.decodeIfPresent(String.self, forKey: .provider)
        model = try values.decodeIfPresent(String.self, forKey: .model)
        status = try values.decodeIfPresent(Int.self, forKey: .status)
        errorKind = try values.decodeIfPresent(String.self, forKey: .errorKind)
        errorCode = try values.decodeIfPresent(String.self, forKey: .errorCode)
        middlewareDecisions = try values.decodeIfPresent(
            [MiddlewareActivityDecision].self, forKey: .middlewareDecisions) ?? []
    }
}

public struct MiddlewareActivityDecision: Codable, Sendable, Equatable {
    public var ruleId: String
    public var ruleName: String?
    public var state: String
    public var action: String?
    public var explanation: String?
    public var suppressed: Bool?
    public var executed: Bool?

    enum CodingKeys: String, CodingKey {
        case state, action, explanation, suppressed, executed
        case ruleId = "rule_id"
        case ruleName = "rule_name"
    }
}

public struct MiddlewareRuntimeStatus: Codable, Sendable, Equatable {
    public var settings: MiddlewareSettings
    public var generation: String?
    public var lastReloadMs: Int64?
    public var rules: [MiddlewareRuleSpecV1]
    public var scripts: [MiddlewareScriptStatus]
    public var leases: [MiddlewareRouteLease]
    public var errors: [String]

    public init(
        settings: MiddlewareSettings = .init(),
        generation: String? = nil,
        lastReloadMs: Int64? = nil,
        rules: [MiddlewareRuleSpecV1] = [],
        scripts: [MiddlewareScriptStatus] = [],
        leases: [MiddlewareRouteLease] = [],
        errors: [String] = []
    ) {
        self.settings = settings
        self.generation = generation
        self.lastReloadMs = lastReloadMs
        self.rules = rules
        self.scripts = scripts
        self.leases = leases
        self.errors = errors
    }

    enum CodingKeys: String, CodingKey {
        case settings, generation, rules, scripts, leases, errors
        case lastReloadMs = "last_reload_ms"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        settings = try values.decodeIfPresent(MiddlewareSettings.self, forKey: .settings) ?? .init()
        generation = try values.decodeIfPresent(String.self, forKey: .generation)
        lastReloadMs = try values.decodeIfPresent(Int64.self, forKey: .lastReloadMs)
        rules = try values.decodeIfPresent([MiddlewareRuleSpecV1].self, forKey: .rules) ?? []
        scripts = try values.decodeIfPresent([MiddlewareScriptStatus].self, forKey: .scripts) ?? []
        leases = try values.decodeIfPresent([MiddlewareRouteLease].self, forKey: .leases) ?? []
        errors = try values.decodeIfPresent([String].self, forKey: .errors) ?? []
    }
}

public struct MiddlewareValidationResponse: Codable, Sendable, Equatable {
    public var valid: Bool
    public var errors: [MiddlewareValidationIssue]
    public var warnings: [MiddlewareValidationIssue]
    public var canonicalRule: MiddlewareRuleSpecV1?

    public init(
        valid: Bool,
        errors: [MiddlewareValidationIssue] = [],
        warnings: [MiddlewareValidationIssue] = [],
        canonicalRule: MiddlewareRuleSpecV1? = nil
    ) {
        self.valid = valid
        self.errors = errors
        self.warnings = warnings
        self.canonicalRule = canonicalRule
    }

    enum CodingKeys: String, CodingKey {
        case valid, errors, warnings
        case canonicalRule = "canonical_rule"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        valid = try values.decodeIfPresent(Bool.self, forKey: .valid) ?? false
        errors = try values.decodeIfPresent([MiddlewareValidationIssue].self, forKey: .errors) ?? []
        warnings = try values.decodeIfPresent([MiddlewareValidationIssue].self, forKey: .warnings) ?? []
        canonicalRule = try values.decodeIfPresent(MiddlewareRuleSpecV1.self, forKey: .canonicalRule)
    }
}

public struct MiddlewareValidationIssue: Codable, Sendable, Equatable, Identifiable {
    public var code: String?
    public var path: String?
    public var message: String

    public var id: String { "\(code ?? "issue"):\(path ?? ""):\(message)" }

    public init(code: String? = nil, path: String? = nil, message: String) {
        self.code = code
        self.path = path
        self.message = message
    }

    enum CodingKeys: String, CodingKey { case code, path, message }

    public init(from decoder: Decoder) throws {
        if let message = try? decoder.singleValueContainer().decode(String.self) {
            code = nil
            path = nil
            self.message = message
            return
        }
        let values = try decoder.container(keyedBy: CodingKeys.self)
        code = try values.decodeIfPresent(String.self, forKey: .code)
        path = try values.decodeIfPresent(String.self, forKey: .path)
        message = try values.decode(String.self, forKey: .message)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encodeIfPresent(code, forKey: .code)
        try values.encodeIfPresent(path, forKey: .path)
        try values.encode(message, forKey: .message)
    }

    public var displayText: String {
        path.map { "\($0): \(message)" } ?? message
    }
}

public struct MiddlewareValidationRequest: Codable, Sendable, Equatable {
    public var rule: MiddlewareRuleSpecV1

    public init(rule: MiddlewareRuleSpecV1) {
        self.rule = rule
    }
}

public struct MiddlewareMutationResponse: Codable, Sendable, Equatable {
    public var generation: String?
    public var rule: MiddlewareRuleSpecV1?

    public init(generation: String? = nil, rule: MiddlewareRuleSpecV1? = nil) {
        self.generation = generation
        self.rule = rule
    }
}

public struct MiddlewareTestRequest: Codable, Sendable, Equatable {
    public var middlewareId: String
    public var fixtureName: String?
    public var traceId: String?

    public init(
        middlewareId: String,
        fixtureName: String? = nil,
        traceId: String? = nil
    ) {
        self.middlewareId = middlewareId
        self.fixtureName = fixtureName
        self.traceId = traceId
    }

    enum CodingKeys: String, CodingKey {
        case middlewareId = "middleware_id"
        case fixtureName = "fixture_name"
        case traceId = "trace_id"
    }
}

public struct MiddlewareTestResponse: Decodable, Sendable, Equatable {
    public var matched: Bool
    public var summary: String?
    public var predicates: [String]
    public var proposedAction: String?
    public var warnings: [String]

    public init(
        matched: Bool,
        summary: String? = nil,
        predicates: [String] = [],
        proposedAction: String? = nil,
        warnings: [String] = []
    ) {
        self.matched = matched
        self.summary = summary
        self.predicates = predicates
        self.proposedAction = proposedAction
        self.warnings = warnings
    }

    enum CodingKeys: String, CodingKey {
        case matched, summary, predicates, warnings, records, decision
        case proposedAction = "proposed_action"
    }

    private struct DryRunRecord: Decodable {
        var state: String
        var explanation: String?
    }

    private struct DryRunDecision: Decodable {
        var decision: String
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let records = try values.decodeIfPresent([DryRunRecord].self, forKey: .records) ?? []
        let matchedRecord = records.first { $0.state == "matched" }
        matched = try values.decodeIfPresent(Bool.self, forKey: .matched) ?? (matchedRecord != nil)
        summary = try values.decodeIfPresent(String.self, forKey: .summary)
            ?? matchedRecord?.explanation
            ?? records.first?.explanation
        predicates = try values.decodeIfPresent([String].self, forKey: .predicates) ?? []
        proposedAction = try values.decodeIfPresent(String.self, forKey: .proposedAction)
            ?? values.decodeIfPresent(DryRunDecision.self, forKey: .decision)?.decision
        warnings = try values.decodeIfPresent([String].self, forKey: .warnings) ?? []
    }
}

// MARK: - Middleware Wizard

public enum MiddlewareWizardErrorKind: String, Codable, Sendable, CaseIterable, Hashable {
    case any = "Any error"
    case authentication = "Authentication"
    case capacity = "Rate / capacity"
    case badRequest = "Bad request"
    case server = "Provider / server"
    case network = "Network"
    case refusal = "Model refusal"

    var errorClasses: [String] {
        switch self {
        case .any: ["auth", "bad_request", "capacity", "client_disconnect", "network", "other", "server"]
        case .authentication: ["auth"]
        case .capacity: ["capacity"]
        case .badRequest: ["bad_request"]
        case .server: ["server"]
        case .network: ["network"]
        case .refusal: []
        }
    }

    var normalizedErrorKinds: [String] {
        self == .refusal ? ["upstream_refusal"] : []
    }
}

public enum MiddlewareWizardAction: String, Codable, Sendable, CaseIterable {
    case retrySame = "Retry the same model"
    case routeExact = "Route to a specific model"
    case routeEquivalent = "Route to an equivalent model"
}

public struct MiddlewareWizardDraft: Sendable, Equatable {
    public static let defaultNoticeTemplate =
        "Alex switched from {from_model} to {to_model}."

    public var name: String
    public var description: String
    public var harnesses: [String]
    public var harnessVersion: String
    public var modelPattern: String
    public var sourceProvider: String
    public var sourceEffort: String
    public var hook: MiddlewareHookPoint
    public var errorKinds: Set<MiddlewareWizardErrorKind>
    public var statusText: String
    public var bodyPhrasesText: String
    public var conditionMode: MiddlewareConditionMode
    public var action: MiddlewareWizardAction
    public var targetModel: String
    public var targetEffort: String
    public var equivalenceClass: String
    public var providerMode: MiddlewareProviderMode
    public var targetProviders: [String]
    public var scope: MiddlewareRouteScope
    public var stableSessionRequired: Bool
    public var ttlSeconds: Int
    public var includeNotice: Bool
    public var notice: String
    public var priority: Int

    public init(
        name: String = "",
        description: String = "",
        harnesses: [String] = [],
        harnessVersion: String = "",
        modelPattern: String = "",
        sourceProvider: String = "",
        sourceEffort: String = "",
        hook: MiddlewareHookPoint = .attemptResult,
        errorKinds: Set<MiddlewareWizardErrorKind> = [.capacity, .server],
        statusText: String = "429, 500-599",
        bodyPhrasesText: String = "",
        conditionMode: MiddlewareConditionMode = .all,
        action: MiddlewareWizardAction = .routeExact,
        targetModel: String = "",
        targetEffort: String = "",
        equivalenceClass: String = "",
        providerMode: MiddlewareProviderMode = .only,
        targetProviders: [String] = [],
        scope: MiddlewareRouteScope = .session,
        stableSessionRequired: Bool = true,
        ttlSeconds: Int = 86_400,
        includeNotice: Bool = false,
        notice: String = "",
        priority: Int = 100
    ) {
        self.name = name
        self.description = description
        self.harnesses = harnesses
        self.harnessVersion = harnessVersion
        self.modelPattern = modelPattern
        self.sourceProvider = sourceProvider
        self.sourceEffort = sourceEffort
        self.hook = hook
        self.errorKinds = errorKinds
        self.statusText = statusText
        self.bodyPhrasesText = bodyPhrasesText
        self.conditionMode = conditionMode
        self.action = action
        self.targetModel = targetModel
        self.targetEffort = targetEffort
        self.equivalenceClass = equivalenceClass
        self.providerMode = providerMode
        self.targetProviders = targetProviders
        self.scope = scope
        self.stableSessionRequired = stableSessionRequired
        self.ttlSeconds = ttlSeconds
        self.includeNotice = includeNotice
        self.notice = notice
        self.priority = priority
    }

    public static var fableToSolExample: MiddlewareWizardDraft {
        .init(
            name: "Fable 5 → GPT-5.6 Sol",
            description: "When Anthropic Fable 5 refuses a request, switch the session to high-effort GPT-5.6 Sol.",
            modelPattern: "claude-fable-5",
            sourceProvider: "anthropic",
            hook: .attemptResult,
            errorKinds: [.refusal],
            statusText: "",
            bodyPhrasesText: "",
            conditionMode: .all,
            action: .routeExact,
            targetModel: "gpt-5.6-sol",
            targetEffort: "high",
            providerMode: .only,
            targetProviders: ["openai"],
            scope: .session,
            includeNotice: false,
            notice: defaultNoticeTemplate,
            priority: 100)
    }

    /// Best-effort projection used when a declarative rule is opened in the
    /// basic wizard. Server validation remains authoritative, and advanced
    /// shapes that cannot be represented are left for the structured preview.
    public init(rule: MiddlewareRuleSpecV1) {
        let reroute = rule.then.reroute
        let selectedErrors = Set(MiddlewareWizardErrorKind.allCases.filter { kind in
            guard kind != .any else { return false }
            if kind == .refusal {
                return !Set(rule.when.errorKinds ?? []).isDisjoint(with: kind.normalizedErrorKinds)
            }
            return !Set(kind.errorClasses).isDisjoint(with: rule.when.errorClasses ?? [])
        })
        self.init(
            name: rule.name,
            description: rule.description ?? "",
            harnesses: rule.when.harnessNames ?? [],
            harnessVersion: (rule.when.harnessVersions ?? []).joined(separator: ", "),
            modelPattern: Self.displayModelPatterns(rule.when.models ?? []).joined(separator: ", "),
            sourceProvider: (rule.when.providers ?? []).joined(separator: ", "),
            sourceEffort: (rule.when.efforts ?? []).first ?? "",
            hook: rule.hook,
            errorKinds: selectedErrors,
            statusText: (rule.when.status ?? []).map(\.displayValue).joined(separator: ", "),
            bodyPhrasesText: (rule.when.bodyContainsAny ?? []).joined(separator: "\n"),
            conditionMode: rule.expression == nil ? .all : .any,
            action: rule.then.retrySameRoute != nil
                ? .retrySame : (reroute?.equivalenceClass == nil ? .routeExact : .routeEquivalent),
            targetModel: reroute?.model ?? "",
            targetEffort: reroute?.effort ?? "",
            equivalenceClass: reroute?.equivalenceClass ?? "",
            providerMode: reroute?.providerMode ?? .any,
            targetProviders: reroute?.providers ?? [],
            scope: reroute?.scope ?? .request,
            stableSessionRequired: rule.when.stableSession ?? true,
            ttlSeconds: reroute?.ttlSeconds ?? 86_400,
            includeNotice: reroute?.notice != nil,
            notice: reroute?.notice ?? Self.defaultNoticeTemplate,
            priority: rule.priority)
    }

    public var localValidationErrors: [String] {
        var errors: [String] = []
        if trimmed(name).isEmpty { errors.append("Enter a name.") }
        if !trimmed(statusText).isEmpty && statusMatchers.count != commaSeparated(statusText).count {
            errors.append("HTTP statuses must be codes, ranges, 4xx, or 5xx.")
        }
        if hook != .attemptResult {
            errors.append("Retry and reroute actions require the Failed attempt hook.")
        }
        if hook != .attemptResult && (!bodyPhrases.isEmpty || !errorKinds.isEmpty || !statusMatchers.isEmpty) {
            errors.append("Failure and body conditions require the Failed attempt hook.")
        }
        if matcherIsEmpty { errors.append("Choose at least one condition.") }
        if action == .routeExact && trimmed(targetModel).isEmpty {
            errors.append("Choose a target model.")
        }
        if action == .routeEquivalent && trimmed(equivalenceClass).isEmpty {
            errors.append("Enter an equivalence class.")
        }
        if providerMode != .any && targetProviders.isEmpty && action != .retrySame {
            errors.append("Choose at least one target provider.")
        }
        if usesSessionScope && !stableSessionRequired {
            errors.append("Session routing requires a stable session identifier policy.")
        }
        if action != .retrySame && includeNotice && trimmed(notice).isEmpty {
            errors.append("Enter the notice Alex should add after a successful reroute.")
        }
        if !(0...10_000).contains(priority) { errors.append("Priority must be between 0 and 10000.") }
        if usesSessionScope && ttlSeconds <= 0 { errors.append("Session route TTL must be positive.") }
        return errors
    }

    public var warnings: [String] {
        var result: [String] = []
        if matcherIsEmpty { result.append("This rule would match every event at its hook.") }
        if !bodyPhrases.isEmpty { result.append("Matching this rule may inspect up to the configured error-body limit.") }
        if usesSessionScope { result.append("The target route is kept only when Alex has a stable, portable session.") }
        if action != .retrySame && includeNotice {
            result.append("A notice can buffer the exceptional fallback response before delivery.")
        }
        return result
    }

    public func makeRule(id: String? = nil) throws -> MiddlewareRuleSpecV1 {
        let errors = localValidationErrors
        guard errors.isEmpty else { throw MiddlewareWizardBuildError.invalid(errors) }

        let normalizedID = id ?? Self.slug(trimmed(name))
        let errorClasses = errorKinds.flatMap(\.errorClasses).sorted()
        let normalizedErrorKinds = errorKinds.flatMap(\.normalizedErrorKinds).sorted()
        let baseMatch = MiddlewareMatchSpec(
            harnessNames: nilIfEmpty(harnesses),
            harnessVersions: nilIfEmpty(commaSeparated(harnessVersion)),
            models: nilIfEmpty(Self.ruleModelPatterns(commaSeparated(modelPattern))),
            efforts: nilIfEmpty(trimmed(sourceEffort)).map { [$0] },
            providers: nilIfEmpty(commaSeparated(sourceProvider)),
            status: conditionMode == .all ? nilIfEmpty(statusMatchers) : nil,
            errorClasses: conditionMode == .all ? nilIfEmpty(errorClasses) : nil,
            errorKinds: conditionMode == .all ? nilIfEmpty(normalizedErrorKinds) : nil,
            bodyContainsAny: conditionMode == .all ? nilIfEmpty(bodyPhrases) : nil,
            stableSession: usesSessionScope ? stableSessionRequired : nil)

        let expression: MiddlewareMatchExpression?
        if conditionMode == .any {
            var alternatives: [MiddlewareMatchExpression] = []
            if !statusMatchers.isEmpty {
                alternatives.append(.conditions(.init(status: statusMatchers)))
            }
            if !errorClasses.isEmpty {
                alternatives.append(.conditions(.init(errorClasses: errorClasses)))
            }
            if !normalizedErrorKinds.isEmpty {
                alternatives.append(.conditions(.init(errorKinds: normalizedErrorKinds)))
            }
            if !bodyPhrases.isEmpty {
                alternatives.append(.conditions(.init(bodyContainsAny: bodyPhrases)))
            }
            expression = alternatives.isEmpty ? nil : .any(alternatives)
        } else {
            expression = nil
        }

        let actionSpec: MiddlewareActionSpec
        switch action {
        case .retrySame:
            actionSpec = .init(retrySameRoute: .init(reason: "Matched \(trimmed(name))"))
        case .routeExact, .routeEquivalent:
            actionSpec = .init(reroute: .init(
                model: action == .routeExact ? trimmed(targetModel) : nil,
                equivalenceClass: action == .routeEquivalent ? trimmed(equivalenceClass) : nil,
                providerMode: providerMode,
                providers: targetProviders,
                scope: scope,
                ttlSeconds: scope == .session ? ttlSeconds : nil,
                notice: includeNotice ? trimmed(notice) : nil,
                effort: nilIfEmpty(trimmed(targetEffort)),
                reason: "Matched \(trimmed(name))",
                maxAttempts: 3,
                requiredCapabilities: .init(portableHistory: usesSessionScope)))
        }

        var capabilities: [String] = []
        if !bodyPhrases.isEmpty { capabilities.append("attempt.read_error_body") }
        capabilities.append("route.override")
        if usesSessionScope { capabilities.append("session.pin") }
        if action != .retrySame && includeNotice { capabilities.append("response.prepend_text") }

        return MiddlewareRuleSpecV1(
            id: normalizedID,
            name: trimmed(name),
            description: nilIfEmpty(trimmed(description)),
            enabled: true,
            priority: priority,
            hook: hook,
            capabilities: capabilities,
            when: baseMatch,
            expression: expression,
            then: actionSpec)
    }

    public var summary: String {
        let harness = harnesses.isEmpty ? "any harness" : Self.naturalList(harnesses.map(Self.titleCase))
        let model = commaSeparated(modelPattern).isEmpty
            ? "any model" : Self.naturalList(commaSeparated(modelPattern))
        let provider = trimmed(sourceProvider).isEmpty ? "any provider" : Self.titleCase(trimmed(sourceProvider))
        let condition: String
        if hook == .attemptResult {
            let labels = errorKinds.map(\.rawValue).sorted()
            condition = labels.isEmpty ? "a failed attempt" : "a \(Self.naturalList(labels).lowercased()) error"
        } else {
            condition = hook == .requestReceived ? "the request arrives" : "the response is ready"
        }
        let result: String
        switch action {
        case .retrySame:
            result = "retry the same route"
        case .routeExact:
            let providers = targetProviders.isEmpty ? "any provider" : Self.naturalList(targetProviders.map(Self.titleCase))
            result = "route to \(trimmed(targetModel)) using \(providers)"
        case .routeEquivalent:
            result = "route within the \(trimmed(equivalenceClass)) equivalence class"
        }
        let sourceEffortSummary = trimmed(sourceEffort).isEmpty
            ? "" : " at \(trimmed(sourceEffort)) effort"
        let targetEffortSummary = trimmed(targetEffort).isEmpty || action == .retrySame
            ? "" : " at \(trimmed(targetEffort)) effort"
        let persistence = usesSessionScope ? " and keep it for the session" : ""
        return "When \(harness) requests \(model)\(sourceEffortSummary) through \(provider) and Alex sees \(condition), \(result)\(targetEffortSummary)\(persistence)."
    }

    public var bodyPhrases: [String] {
        bodyPhrasesText
            .components(separatedBy: .newlines)
            .flatMap { $0.components(separatedBy: ",") }
            .map { trimmed($0) }
            .filter { !$0.isEmpty }
    }

    public var statusMatchers: [MiddlewareStatusMatcher] {
        commaSeparated(statusText).compactMap { value in
            if let exact = Int(value), (100...599).contains(exact) { return .exact(exact) }
            let lower = value.lowercased()
            if lower == "4xx" || lower == "5xx" { return .pattern(lower) }
            let parts = lower.split(separator: "-", maxSplits: 1).compactMap { Int($0) }
            if parts.count == 2, parts[0] >= 100, parts[1] <= 599, parts[0] <= parts[1] {
                return .pattern("\(parts[0])-\(parts[1])")
            }
            return nil
        }
    }

    private var matcherIsEmpty: Bool {
        harnesses.isEmpty
            && trimmed(harnessVersion).isEmpty
            && commaSeparated(modelPattern).isEmpty
            && trimmed(sourceProvider).isEmpty
            && trimmed(sourceEffort).isEmpty
            && errorKinds.isEmpty
            && statusMatchers.isEmpty
            && bodyPhrases.isEmpty
    }

    private var usesSessionScope: Bool {
        action != .retrySame && scope == .session
    }

    private func commaSeparated(_ text: String) -> [String] {
        text.split(separator: ",").map { trimmed(String($0)) }.filter { !$0.isEmpty }
    }

    private func trimmed(_ value: String) -> String {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func nilIfEmpty<T>(_ value: [T]) -> [T]? { value.isEmpty ? nil : value }
    private func nilIfEmpty(_ value: String) -> String? { value.isEmpty ? nil : value }

    private static func slug(_ value: String) -> String {
        let lowered = value.lowercased()
        let components = lowered.components(separatedBy: CharacterSet.alphanumerics.inverted)
        let slug = components.filter { !$0.isEmpty }.joined(separator: "-")
        return slug.isEmpty ? "middleware-rule" : slug
    }

    private static func ruleModelPatterns(_ values: [String]) -> [String] {
        unique(values.map { value in
            switch value.lowercased() {
            case "fable-5": "claude-fable-5"
            default: value
            }
        })
    }

    private static func displayModelPatterns(_ values: [String]) -> [String] {
        unique(values)
    }

    private static func unique(_ values: [String]) -> [String] {
        var seen = Set<String>()
        return values.filter { seen.insert($0).inserted }
    }

    private static func naturalList(_ values: [String]) -> String {
        switch values.count {
        case 0: ""
        case 1: values[0]
        case 2: values.joined(separator: " or ")
        default: values.dropLast().joined(separator: ", ") + ", or " + (values.last ?? "")
        }
    }

    private static func titleCase(_ value: String) -> String {
        switch value.lowercased() {
        case "openai": "OpenAI"
        case "anthropic": "Anthropic"
        case "codex": "Codex"
        case "claude": "Claude"
        case "pi": "Pi"
        default: value.capitalized
        }
    }
}

public enum MiddlewareWizardBuildError: Error, LocalizedError, Sendable, Equatable {
    case invalid([String])

    public var errorDescription: String? {
        switch self {
        case let .invalid(errors): errors.joined(separator: " ")
        }
    }
}
