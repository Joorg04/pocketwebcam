import SwiftUI

/// Top row: connection pill, live stats, thermal warning, settings button.
struct StatusBar: View {
    @EnvironmentObject private var model: AppModel
    @Binding var showSettings: Bool

    var body: some View {
        HStack(spacing: 8) {
            ConnectionPill()
            ThermalBadge()
            Spacer()
            CircleButton(systemImage: "gearshape.fill", label: "Settings") {
                showSettings = true
            }
        }
    }
}

/// "Hot" / "Too hot" warning; empty while the phone is cool. `compact` shows only the icon.
struct ThermalBadge: View {
    @EnvironmentObject private var model: AppModel
    var compact = false

    var body: some View {
        if model.thermal == .serious || model.thermal == .critical {
            let text = model.thermal == .critical ? "Too hot" : "Hot"
            let color: Color = model.thermal == .critical ? .red : .orange
            if compact {
                Image(systemName: "thermometer.high")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(color)
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(.ultraThinMaterial))
                    .accessibilityLabel(text)
            } else {
                Label(text, systemImage: "thermometer.high")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(color)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(.ultraThinMaterial, in: Capsule())
            }
        }
    }
}

/// Icon-sized connection status for the landscape side strip: green when live, yellow while waiting.
struct StatusDot: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        let color: Color = model.connectedTo == nil ? .yellow : .green
        Circle()
            .fill(color)
            .frame(width: 10, height: 10)
            .shadow(color: color, radius: 4)
            .frame(width: 40, height: 40)
            .background(Circle().fill(.ultraThinMaterial))
            .accessibilityElement()
            .accessibilityLabel(model.connectedTo.map { "Live on \($0)" } ?? "Waiting for computer")
    }
}

private struct ConnectionPill: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(model.connectedTo == nil ? Color.yellow : Color.green)
                .frame(width: 8, height: 8)
                .shadow(color: model.connectedTo == nil ? .yellow : .green, radius: 4)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.connectedTo.map { "Live on \($0)" } ?? "Waiting for computer")
                    .font(.footnote.weight(.semibold))
                    .lineLimit(1)
                if model.connectedTo != nil {
                    HStack(spacing: 6) {
                        Text("\(model.streamFPS) fps · \(model.streamMbps, specifier: "%.1f") Mbps")
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.secondary)
                        if model.micOn {
                            MicLevel(level: model.micLevel)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial, in: Capsule())
        .accessibilityElement(children: .combine)
    }
}

/// Small mic icon that brightens with the sound level, so you can see the phone hears you.
struct MicLevel: View {
    let level: Float

    var body: some View {
        // Speech peaks sit around 0.1 to 0.5; a square root spreads that range out.
        let amount = Double(min(1, sqrt(level) * 1.4))
        Image(systemName: "mic.fill")
            .font(.caption2)
            .foregroundStyle(Color.green.opacity(0.35 + 0.65 * amount))
            .animation(.linear(duration: 0.1), value: amount)
            .accessibilityLabel("Microphone on")
    }
}

struct CircleButton: View {
    let systemImage: String
    let label: String
    var active = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(active ? .black : .white)
                .frame(width: 40, height: 40)
                .background {
                    if active {
                        Circle().fill(.yellow)
                    } else {
                        Circle().fill(.ultraThinMaterial)
                    }
                }
        }
        .accessibilityLabel(label)
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}
