import Foundation

/// A parsed, orderable Alexandria version. This is a **1:1 port** of the
/// daemon's Rust `ParsedVersion`/`compare_versions`
/// (`crates/alex/src/selfupdate.rs`) so the app (Sparkle) and the daemon can
/// never disagree about which build is newer (B4). Keep the two in lockstep:
/// any change here must be mirrored in Rust and vice-versa, and both test
/// suites cover the same real version strings.
///
/// Ordering: the dotted numeric core dominates; a final release outranks all
/// of its pre-releases; among pre-releases `alpha < beta < rc`; a higher
/// pre-release number is newer. Trailing zeros are insignificant
/// (`0.1.0` == `0.1`), a leading `v` is tolerated, and `+build` metadata is
/// ignored.
public struct AlexVersion: Equatable, Sendable {
    /// Dotted numeric core with trailing zeros trimmed.
    public let release: [UInt64]
    /// Pre-release stage (see the `stage*` constants); stable is the highest.
    public let stage: Int
    /// The pre-release number (`beta.3` → 3); 0 when absent.
    public let preNum: UInt64

    // Ordered stages — unknown pre-release labels sort below the recognized
    // ones, and a stable release outranks every pre-release.
    public static let stageUnknownPre = 0
    public static let stageAlpha = 1
    public static let stageBeta = 2
    public static let stageRC = 3
    public static let stageStable = 4

    public var isStable: Bool { stage == Self.stageStable }

    /// Trim a leading `v`/`V` and surrounding whitespace.
    static func normalizeTag(_ version: String) -> Substring {
        var s = version[...]
        while let first = s.first, first == " " || first == "\t" { s = s.dropFirst() }
        while let last = s.last, last == " " || last == "\t" { s = s.dropLast() }
        if let first = s.first, first == "v" || first == "V" { s = s.dropFirst() }
        return s
    }

    /// First contiguous run of digits in `s`, or 0 when there is none.
    static func firstNumber<S: StringProtocol>(_ s: S) -> UInt64 {
        var digits = ""
        var started = false
        for ch in s {
            if ch.isNumber {
                digits.append(ch)
                started = true
            } else if started {
                break
            }
        }
        return UInt64(digits) ?? 0
    }

    /// Robust parse mirroring the Rust `parse_version`. Returns nil only when
    /// the numeric core is genuinely absent or non-numeric.
    public static func parse(_ version: String) -> AlexVersion? {
        var core = normalizeTag(version)
        // Drop build metadata: everything from the first '+'.
        if let plus = core.firstIndex(of: "+") { core = core[..<plus] }
        let base: Substring
        let pre: Substring?
        if let dash = core.firstIndex(of: "-") {
            base = core[..<dash]
            pre = core[core.index(after: dash)...]
        } else {
            base = core
            pre = nil
        }
        if base.isEmpty { return nil }
        var release: [UInt64] = []
        for part in base.split(separator: ".", omittingEmptySubsequences: false) {
            guard let n = UInt64(part) else { return nil }
            release.append(n)
        }
        // Trim trailing zeros so 0.1.0 == 0.1 and 0.1.24 == 0.1.24.0.
        while release.count > 1 && release.last == 0 { release.removeLast() }

        let stage: Int
        let preNum: UInt64
        if let pre {
            let low = pre.lowercased()
            if low.hasPrefix("rc") {
                stage = stageRC
            } else if low.hasPrefix("beta") {
                stage = stageBeta
            } else if low.hasPrefix("alpha") {
                stage = stageAlpha
            } else {
                stage = stageUnknownPre
            }
            preNum = firstNumber(pre)
        } else {
            stage = stageStable
            preNum = 0
        }
        return AlexVersion(release: release, stage: stage, preNum: preNum)
    }

    /// Order `self` against `other` (release core, then stage, then number).
    public func ordered(vs other: AlexVersion) -> ComparisonResult {
        let maxLen = max(release.count, other.release.count)
        for i in 0..<maxLen {
            let l = i < release.count ? release[i] : 0
            let r = i < other.release.count ? other.release[i] : 0
            if l != r { return l < r ? .orderedAscending : .orderedDescending }
        }
        if stage != other.stage { return stage < other.stage ? .orderedAscending : .orderedDescending }
        if preNum != other.preNum { return preNum < other.preNum ? .orderedAscending : .orderedDescending }
        return .orderedSame
    }

    /// Compare two version strings. When both parse, the structural order is
    /// used; otherwise we fall back to a numeric-aware string compare rather
    /// than claiming equality (which would hide an available update). This is
    /// the comparator handed to Sparkle so it matches the daemon exactly (B4).
    public static func compare(_ a: String, _ b: String) -> ComparisonResult {
        if let pa = parse(a), let pb = parse(b) {
            return pa.ordered(vs: pb)
        }
        return a.compare(b, options: .numeric)
    }
}
