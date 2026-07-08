import SwiftUI
import AlexandriaBarCore

struct LimitsCardView: View {
    let limits: [ProviderLimits]
    let warnPct: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(limits, id: \.provider) { provider in
                providerSection(provider)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .frame(width: 320, alignment: .leading)
    }

    @ViewBuilder
    private func providerSection(_ provider: ProviderLimits) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(ProviderInfo.displayName(provider.provider))
                    .font(.system(size: 12, weight: .semibold))
                Spacer()
                if let plan = provider.plan {
                    Text(plan)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            }
            if let error = provider.error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .lineLimit(2)
            }
            ForEach(provider.windows ?? [], id: \.window) { window in
                windowRow(window)
            }
            if provider.windows?.isEmpty != false, let requests = provider.requests {
                countRow("requests", requests)
                if let tokens = provider.tokens {
                    countRow("tokens", tokens)
                }
            }
        }
    }

    @ViewBuilder
    private func windowRow(_ window: LimitWindow) -> some View {
        let pct = window.usedPct ?? 0
        HStack(spacing: 8) {
            Text(window.window)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.primary.opacity(0.12))
                    Capsule()
                        .fill(barColor(pct))
                        .frame(width: max(3, geo.size.width * min(pct, 100) / 100))
                }
            }
            .frame(height: 6)
            Text("\(Int(pct))%")
                .font(.system(size: 10, design: .monospaced))
                .frame(width: 34, alignment: .trailing)
            Text(window.resetsDate.map { Format.countdown(to: $0) } ?? "")
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
                .frame(width: 52, alignment: .trailing)
        }
    }

    @ViewBuilder
    private func countRow(_ label: String, _ pair: CountPair) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .leading)
            Text("\(pair.remaining ?? 0) / \(pair.limit ?? 0) remaining")
                .font(.system(size: 10, design: .monospaced))
        }
    }

    private func barColor(_ pct: Double) -> Color {
        if pct >= warnPct { return .red }
        if pct >= warnPct * 0.75 { return .orange }
        return .green
    }
}
