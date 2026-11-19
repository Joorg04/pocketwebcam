import SwiftUI

/// "4K · 60 · 10 Mbps" summary. Tapping it opens or closes the stream options.
/// `compact` stacks the values in a 44 pt wide tile for the landscape side strip.
struct FormatChip: View {
    @EnvironmentObject private var model: AppModel
    @Binding var expanded: Bool
    var compact = false

    var body: some View {
        let resolution = resolutionName(model.config.width, model.config.height)
        let mbps = model.config.bitrate / 1_000_000
        let fill = expanded ? AnyShapeStyle(Color.yellow) : AnyShapeStyle(Color.white.opacity(0.12))
        Button {
            expanded.toggle()
        } label: {
            if compact {
                VStack(spacing: 3) {
                    Text(resolution).font(.caption.weight(.bold))
                    Text("\(model.config.fps)").font(.caption.weight(.semibold))
                    Text("\(mbps)M").font(.caption2.weight(.semibold))
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.bold))
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                }
                .monospacedDigit()
                .foregroundStyle(expanded ? .black : .white)
                .frame(width: 44)
                .padding(.vertical, 8)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(fill))
            } else {
                HStack(spacing: 6) {
                    Text("\(resolution) · \(model.config.fps) · \(mbps) Mbps")
                        .font(.footnote.weight(.semibold).monospacedDigit())
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.bold))
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                }
                .foregroundStyle(expanded ? .black : .white)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(Capsule().fill(fill))
            }
        }
        .accessibilityLabel("Stream options")
        .accessibilityValue(expanded ? "Open" : "Closed")
    }
}

/// Resolution, frame rate and quality. Shown only while the format chip is open;
/// the video frame shrinks to make room and grows back when it closes.
struct OptionsPanel: View {
    @EnvironmentObject private var model: AppModel

    private var fpsOptions: [Int] {
        model.currentCamera?.formats
            .first { $0.width == model.config.width && $0.height == model.config.height }?
            .fps ?? [30]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            OptionRow(title: "Resolution") {
                ForEach(model.currentCamera?.formats ?? [], id: \.self) { f in
                    OptionChip(text: resolutionName(f.width, f.height),
                               selected: f.width == model.config.width && f.height == model.config.height) {
                        model.setResolution(f)
                    }
                }
            }
            OptionRow(title: "Frame rate") {
                ForEach(fpsOptions, id: \.self) { fps in
                    OptionChip(text: "\(fps)", selected: fps == model.config.fps) {
                        model.setFPS(fps)
                    }
                }
            }
            OptionRow(title: "Quality") {
                ForEach(Quality.allCases) { q in
                    OptionChip(text: "\(q.rawValue) · \(q.bitrate(for: model.config) / 1_000_000) Mbps",
                               selected: q == model.quality) {
                        model.setQuality(q)
                    }
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}

private struct OptionRow<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) { content }
            }
        }
    }
}

private struct OptionChip: View {
    let text: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(text)
                .font(.footnote.weight(.semibold).monospacedDigit())
                .foregroundStyle(selected ? .black : .white)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(Capsule().fill(selected ? AnyShapeStyle(Color.yellow) : AnyShapeStyle(Color.white.opacity(0.12))))
        }
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// Lens buttons: a row centered under the video, or a column beside it in landscape.
struct LensPicker: View {
    @EnvironmentObject private var model: AppModel
    var vertical = false

    var body: some View {
        let layout = vertical ? AnyLayout(VStackLayout(spacing: 10)) : AnyLayout(HStackLayout(spacing: 10))
        layout {
            ForEach(model.capabilities.cameras) { cam in
                let selected = cam.id == model.config.camera
                Button {
                    model.selectCamera(cam.id)
                } label: {
                    Group {
                        if cam.position == "front" {
                            Image(systemName: "person.crop.circle")
                                .font(.system(size: 15, weight: .semibold))
                        } else {
                            Text(model.lensLabel(cam))
                                .font(.system(size: 13, weight: .bold).monospacedDigit())
                        }
                    }
                    .foregroundStyle(selected ? .yellow : .white)
                    .frame(width: selected ? 44 : 38, height: selected ? 44 : 38)
                    .background(Circle().fill(.white.opacity(selected ? 0.18 : 0.1)))
                }
                .accessibilityLabel(cam.name)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .frame(width: vertical ? 44 : nil, height: vertical ? nil : 44)
        .animation(.spring(duration: 0.25), value: model.config.camera)
    }
}

/// Flashlight, mirror and sleep, stacked along the right edge of the video.
struct ToggleColumn: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: 8) {
            if model.connectedTo != nil {
                MicButton()
            }
            if model.currentCamera?.features.contains("torch") == true {
                CircleButton(systemImage: model.config.torch ? "flashlight.on.fill" : "flashlight.off.fill",
                             label: "Torch", active: model.config.torch) {
                    model.setTorch(!model.config.torch)
                }
            }
            CircleButton(systemImage: "arrow.left.and.right.righttriangle.left.righttriangle.right",
                         label: "Mirror", active: model.config.mirror) {
                model.setMirror(!model.config.mirror)
            }
            CircleButton(systemImage: "moon.fill", label: "Dim screen") {
                model.dimmed = true
            }
        }
    }
}

/// Mic on/off. With the permission denied it opens Settings, the only place to fix it.
private struct MicButton: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        if model.micPermission == .denied {
            CircleButton(systemImage: "mic.slash.fill", label: "Microphone not allowed. Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
            }
        } else {
            CircleButton(systemImage: model.micOn ? "mic.fill" : "mic.slash.fill",
                         label: "Microphone", active: model.micOn) {
                model.setMicrophone(!model.micOn)
            }
        }
    }
}

func resolutionName(_ w: Int, _ h: Int) -> String {
    switch (w, h) {
    case (3840, 2160): "4K"
    case (1920, 1080): "1080p"
    case (1280, 720): "720p"
    default: "\(h)p"
    }
}
