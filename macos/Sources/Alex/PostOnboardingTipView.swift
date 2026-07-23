import SwiftUI
import AlexCore

/// Shown once under the menu-bar icon when onboarding completes: Alex lives
/// in the menu bar, and the dashboard and settings are one click away.
struct PostOnboardingTipView: View {
    let openDashboard: () -> Void
    let openSettings: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "sparkles")
                    .font(.system(size: 15))
                    .foregroundStyle(AlexTheme.Colors.primary)
                Text("Alex lives up here")
                    .font(.system(size: 14, weight: .semibold))
            }
            Text("Click the menu-bar icon anytime to see providers, harnesses, usage, and recent traces.")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                PillButton(
                    title: "Open Trace Browser", variant: .solidAccent,
                    systemImage: "magnifyingglass"
                ) { openDashboard() }
                PillButton(title: "Settings", variant: .bordered, systemImage: "gearshape") {
                    openSettings()
                }
            }
        }
        .padding(16)
        .frame(width: 320)
    }
}
