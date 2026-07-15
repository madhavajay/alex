import AppKit
import SwiftUI

/// App-wide design tokens derived from the Trace Browser design spec
/// (ui/Enhance Trace Browser UI). Dark-mode values match the mock exactly;
/// colors map onto Apple dynamic system colors where they coincide so a
/// light appearance resolves sensibly without a rewrite.
enum AlexTheme {
    enum Colors {
        // Surfaces
        static let background = dynamic(light: 0xF2F2F7, dark: 0x1C1C1E)
        static let card = dynamic(light: 0xFFFFFF, dark: 0x2C2C2E)
        static let secondaryFill = dynamic(light: 0xD1D1D6, dark: 0x3A3A3C)
        /// Mid surface between bg and card (Accounts/Create Settings card #28282a).
        static let surfaceRaised = dynamic(light: 0xFFFFFF, dark: 0x28282A)
        /// Table header / help band (Dario mock #141414). Light value derived.
        static let surfaceSunken = dynamic(light: 0xE9E9EB, dark: 0x141414)
        /// Console/log background (Dario mock #111112). Light value derived.
        static let consoleBackground = dynamic(light: 0xF7F7F9, dark: 0x111112)

        // Text tiers (mock: #e5e5ea / #aeaeb2 / #8e8e93 / #636366 / #48484a / #3a3a3c)
        static let foreground = dynamic(light: 0x1C1C1E, dark: 0xE5E5EA)
        static let textSecondary = dynamic(light: 0x636366, dark: 0xAEAEB2)
        static let mutedForeground = Color(nsColor: .systemGray)  // #8e8e93 both modes
        static let textTertiary = dynamic(light: 0xAEAEB2, dark: 0x636366)
        static let textFaint = dynamic(light: 0xC7C7CC, dark: 0x48484A)
        static let textFaintest = dynamic(light: 0xD1D1D6, dark: 0x3A3A3C)

        // Semantic (system colors resolve to the mock's dark values)
        static let primary = Color(nsColor: .systemBlue)  // #0a84ff dark
        static let primaryBright = dynamic(light: 0x0071E3, dark: 0x409CFF)
        static let indigo = Color(nsColor: .systemIndigo)  // #5e5ce6 dark
        static let destructive = Color(nsColor: .systemRed)  // #ff453a dark
        static let success = Color(nsColor: .systemGreen)  // #30d158 dark
        static let warning = Color(nsColor: .systemYellow)  // #ffd60a dark
        /// Update banner / slow-idle / "upd" badge orange (#ff9f0a dark,
        /// #ff9500 light) — the mocks distinguish this from the running-yellow.
        static let warningOrange = Color(nsColor: .systemOrange)
        static let purple = Color(nsColor: .systemPurple)  // #bf5af2 dark / #af52de light
        static let teal = dynamic(light: 0x32ADE6, dark: 0x5AC8FA)  // gpt badge
        /// Menu-highlight blue used by the mock's flyout rows (menu App.tsx:382-401).
        static let menuHighlight = dynamic(light: 0x0057D8, dark: 0x0057D8)
        /// Focus ring (Create Settings --ring rgba(10,132,255,0.4)).
        static let focusRing = Color(nsColor: .systemBlue).opacity(0.4)

        /// Chart series palette (Accounts --chart-1..5 + OpenRouter #ff6961).
        /// Fixed values in both appearances so series colors stay recognizable.
        static let chartPalette: [Color] = [
            dynamic(light: 0x0A84FF, dark: 0x0A84FF),
            dynamic(light: 0x34C759, dark: 0x34C759),
            dynamic(light: 0xFF9F0A, dark: 0xFF9F0A),
            dynamic(light: 0xFF453A, dark: 0xFF453A),
            dynamic(light: 0xBF5AF2, dark: 0xBF5AF2),
            dynamic(light: 0xFF6961, dark: 0xFF6961),
        ]

        /// JSON syntax palette (Trace Browser shared.tsx:380-388). Dark values
        /// match the mock exactly; light values are darkened for contrast.
        enum Json {
            static let key = dynamic(light: 0x33708E, dark: 0x79B8D4)
            static let string = dynamic(light: 0x4A7A3E, dark: 0x87BD78)
            static let number = dynamic(light: 0x9C5A28, dark: 0xD49668)
            static let boolean = dynamic(light: 0x7C4FA8, dark: 0xB48ADE)
            static let null = dynamic(light: 0x63636E, dark: 0x7A7A9A)
            static let punctuation = dynamic(light: 0xB8B8C2, dark: 0x3E3E4A)
        }

        // Neutral overlays — mock uses rgba(255,255,255,x) on dark; the light
        // appearance mirrors with black at the same opacity.
        static let border = overlay(0.08)
        static let borderStrong = overlay(0.14)
        static let cardBorder = overlay(0.07)
        static let avatarBorder = overlay(0.10)
        static let hairline = overlay(0.05)
        static let divider = overlay(0.04)
        static let surfaceFaint = overlay(0.04)
        static let surfaceHover = overlay(0.06)
        static let surfaceActive = overlay(0.10)
        static let selectionWash = overlay(0.03)

        // Message bubbles
        static let userBubble = dynamicRGBA(
            light: (229, 229, 234, 0.80), dark: (44, 44, 46, 0.80))
        static let userBubbleSelected = dynamicRGBA(
            light: (209, 209, 214, 0.95), dark: (52, 52, 54, 0.95))
        static let userBubbleText = dynamic(light: 0x3A3A3C, dark: 0xC7C7CC)
        static let assistantBubble = dynamicRGBA(
            light: (10, 84, 160, 0.10), dark: (10, 84, 160, 0.16))
        static let assistantBubbleSelected = dynamicRGBA(
            light: (10, 84, 160, 0.20), dark: (10, 84, 160, 0.28))
        static let assistantBubbleText = dynamic(light: 0x0A3A75, dark: 0xDDE8FF)

        /// Black wash regardless of appearance (mock rgba(0,0,0,x); the settings
        /// sidebar family). Unlike `overlay`, this darkens in both modes.
        static func blackOverlay(_ opacity: CGFloat) -> Color {
            Color(nsColor: NSColor(name: nil) { _ in
                NSColor(white: 0, alpha: opacity)
            })
        }

        /// Settings-sidebar wash (mock rgba(0,0,0,0.25); light derived).
        static let sidebarWash = dynamicRGBA(
            light: (0, 0, 0, 0.04), dark: (0, 0, 0, 0.25))

        static func overlay(_ opacity: CGFloat) -> Color {
            Color(nsColor: NSColor(name: nil) { appearance in
                NSColor(white: isDark(appearance) ? 1 : 0, alpha: opacity)
            })
        }

        static func dynamic(light: UInt32, dark: UInt32) -> Color {
            Color(nsColor: NSColor(name: nil) { appearance in
                nsColor(hex: isDark(appearance) ? dark : light)
            })
        }

        static func dynamicRGBA(
            light: (CGFloat, CGFloat, CGFloat, CGFloat),
            dark: (CGFloat, CGFloat, CGFloat, CGFloat)
        ) -> Color {
            Color(nsColor: NSColor(name: nil) { appearance in
                let (r, g, b, a) = isDark(appearance) ? dark : light
                return NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: a)
            })
        }

        private static func isDark(_ appearance: NSAppearance) -> Bool {
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        }

        private static func nsColor(hex: UInt32, alpha: CGFloat = 1) -> NSColor {
            NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: alpha)
        }
    }

    /// Centralized provider brand colors. `accent` drives sidebar dots, bar
    /// fills, and count badges (Accounts App.tsx:14-21); `chipText` /
    /// `chipBackground` drive the Trace Browser 17×17 tinted provider chip
    /// (shared.tsx:196-209); `authAccent` is the auth-window tint
    /// (#D4693A "Anthropic orange", Auth App.tsx:7-9).
    struct ProviderBrand {
        let accent: Color
        let chipText: Color
        let chipBackground: Color
        let authAccent: Color

        static func brand(for provider: String) -> ProviderBrand {
            switch provider.lowercased() {
            case "anthropic", "claude":
                ProviderBrand(
                    accent: Colors.dynamic(light: 0xBF5AF2, dark: 0xBF5AF2),
                    chipText: Colors.dynamic(light: 0xC65500, dark: 0xFF9040),
                    chipBackground: Color(red: 1, green: 107 / 255, blue: 0).opacity(0.18),
                    authAccent: Colors.dynamic(light: 0xD4693A, dark: 0xD4693A))
            case "openai", "codex":
                ProviderBrand(
                    accent: Colors.dynamic(light: 0x0A84FF, dark: 0x0A84FF),
                    chipText: Colors.dynamic(light: 0x0D8A61, dark: 0x10B981),
                    chipBackground: Color(red: 16 / 255, green: 185 / 255, blue: 129 / 255)
                        .opacity(0.18),
                    authAccent: Colors.dynamic(light: 0x10B981, dark: 0x10B981))
            case "gemini", "google":
                ProviderBrand(
                    accent: Colors.dynamic(light: 0x34C759, dark: 0x34C759),
                    chipText: Colors.dynamic(light: 0x2A6FDB, dark: 0x4285F4),
                    chipBackground: Color(red: 66 / 255, green: 133 / 255, blue: 244 / 255)
                        .opacity(0.18),
                    authAccent: Colors.dynamic(light: 0x4285F4, dark: 0x4285F4))
            case "amp":
                tinted(0xFF9F0A)
            case "xai", "grok":
                tinted(0xFF453A)
            case "openrouter":
                tinted(0xFF6961)
            case "cursor":
                tinted(0x8E5CFF)
            default:
                tinted(0x8E8E93)
            }
        }

        private static func tinted(_ hex: UInt32) -> ProviderBrand {
            let color = Colors.dynamic(light: hex, dark: hex)
            return ProviderBrand(
                accent: color,
                chipText: color,
                chipBackground: color.opacity(0.18),
                authAccent: color)
        }
    }

    /// Per-harness brand-tile palette (Create Settings App.tsx:90-123): the
    /// tile the harness logo sits on so dark logos stay legible, plus the
    /// inset that keeps each logo optically balanced on a 32px tile.
    enum HarnessBrand {
        static func tileBackground(for harness: String) -> Color {
            switch harness {
            case "claude": Colors.dynamic(light: 0xF0EBE3, dark: 0xF0EBE3)
            case "codex": Colors.dynamic(light: 0xFFFFFF, dark: 0xFFFFFF)
            case "amp": Colors.dynamic(light: 0x1A1A1A, dark: 0x1A1A1A)
            case "pi": Colors.dynamic(light: 0x09090B, dark: 0x09090B)
            case "grok": Colors.dynamic(light: 0x0D0D0D, dark: 0x0D0D0D)
            case "gemini": Colors.dynamic(light: 0xFFFFFF, dark: 0xFFFFFF)
            case "opencode": Colors.dynamic(light: 0x000000, dark: 0x000000)
            // Transparent magenta mark needs an opaque light tile for legibility.
            case "pydantic-ai": Colors.dynamic(light: 0xFFFFFF, dark: 0xFFFFFF)
            default: Colors.overlay(0.08)
            }
        }

        static func tilePadding(for harness: String) -> CGFloat {
            harness == "amp" ? 6 : 5
        }
    }

    /// Shared layout constants from the mocks (shared.tsx:34-36 + row specs).
    enum Metrics {
        /// Panel header height (all Trace Browser / Dario panels).
        static let panelHeaderHeight: CGFloat = 48
        /// Filter bar height under panel headers.
        static let filterRowHeight: CGFloat = 40
        /// Compact session list row height.
        static let listRowHeight: CGFloat = 30
        /// Session list column header row height.
        static let columnHeaderHeight: CGFloat = 24
        /// Panel footer / status strip height.
        static let footerHeight: CGFloat = 28
    }

    enum Spacing {
        static let xxs: CGFloat = 2
        static let xs: CGFloat = 4
        static let sm: CGFloat = 6
        static let md: CGFloat = 8
        static let ml: CGFloat = 10
        static let lg: CGFloat = 12
        static let xl: CGFloat = 16
    }

    enum Radius {
        static let xs: CGFloat = 4
        static let sm: CGFloat = 6
        static let md: CGFloat = 8  // tool-call cards
        static let lg: CGFloat = 10  // spec --radius 0.625rem
        static let xl: CGFloat = 12  // subagent cards
        static let bubble: CGFloat = 16
    }

    enum Fonts {
        static func mono(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
            .system(size: size, weight: weight, design: .monospaced)
        }

        /// 10px mono — timestamps, durations, token counts.
        static let metaMicro = mono(10)
        /// 10.5px mono — expanded tool input/output panes.
        static let metaMono = mono(10.5)
        /// 11px mono medium — tool names, argument previews.
        static let metaLabel = mono(11, weight: .medium)
        /// 9px mono — status chips.
        static let chipMono = mono(9)
        /// 11px semibold — role labels ("User / Harness", "Model", "SUBAGENT").
        static let roleLabel = Font.system(size: 11, weight: .semibold)
        /// 10px medium — small buttons and tab labels.
        static let smallControl = Font.system(size: 10, weight: .medium)
        /// 13px — spec base body size.
        static let body = Font.system(size: 13)
        /// 12px — message bubble content.
        static let bubbleBody = Font.system(size: 12)
        /// 14px mono semibold — menu stats bar values.
        static let statValue = mono(14, weight: .semibold)
        /// 15px semibold — settings panel titles ("General", "Harnesses").
        static let panelTitle = Font.system(size: 15, weight: .semibold)
        /// 22px mono semibold — auth device-flow code.
        static let authCode = mono(22, weight: .semibold)
    }
}

extension View {
    /// Standard card container (Accounts/Settings): radius 12, faint surface,
    /// 1px hairline border (§1.17 of the design spec).
    func alexCard(
        radius: CGFloat = AlexTheme.Radius.xl,
        background: Color = AlexTheme.Colors.surfaceFaint,
        border: Color = AlexTheme.Colors.cardBorder
    ) -> some View {
        self
            .background(RoundedRectangle(cornerRadius: radius).fill(background))
            .overlay(RoundedRectangle(cornerRadius: radius).strokeBorder(border))
    }
}
