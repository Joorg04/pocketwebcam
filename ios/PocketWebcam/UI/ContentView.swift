import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage("onboardingDone") private var onboardingDone = false
    @State private var showSettings = false
    @State private var showOptions = false
    @State private var showScanner = false
    @Environment(\.verticalSizeClass) private var verticalSizeClass

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()

            Group {
                if verticalSizeClass == .compact {
                    // The side safe areas already leave a margin; every point of width goes to the video.
                    landscape
                        .padding(.vertical, 4)
                } else {
                    portrait
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                }
            }
            .animation(.easeInOut(duration: 0.25), value: showOptions)

            if model.dimmed {
                DimmedOverlay()
                    .transition(.opacity)
            }

            if let prompt = model.pairing {
                PairingCard(prompt: prompt)
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.25), value: model.dimmed)
        .animation(.easeInOut(duration: 0.2), value: model.pairing)
        .sheet(isPresented: $showSettings) {
            SettingsView(onPair: openScanner)
        }
        .sheet(isPresented: $showScanner) {
            PairScannerView()
        }
        .onChange(of: model.pairing) { _, prompt in
            // A computer asking to pair by code needs the screen, even over Settings.
            if case .code = prompt { showSettings = false }
        }
        .fullScreenCover(isPresented: .init(get: { !onboardingDone }, set: { onboardingDone = !$0 })) {
            OnboardingView { onboardingDone = true }
        }
    }

    /// Opens the scanner, after Settings has gone away if it was open.
    private func openScanner() {
        if showSettings {
            showSettings = false
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { showScanner = true }
        } else {
            showScanner = true
        }
    }

    /// Shown while no computer is connected and none is paired yet.
    private var needsPairing: Bool { model.connectedTo == nil && model.pairedComputers.isEmpty }

    /// Upright: status and format chip on top, video in the middle, lenses below.
    private var portrait: some View {
        VStack(spacing: 10) {
            StatusBar(showSettings: $showSettings)
            if needsPairing {
                Button(action: openScanner) {
                    Label("Pair with a computer", systemImage: "qrcode.viewfinder")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            }
            HStack {
                FormatChip(expanded: $showOptions)
                Spacer()
            }
            if showOptions {
                OptionsPanel()
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
            VideoFrame()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            LensPicker()
        }
    }

    /// Sideways the screen is wider than the video, so the video takes the full height
    /// and only icon-wide strips remain: status, format and settings on the left,
    /// lenses on the right. The options panel squeezes in beside the video while open.
    private var landscape: some View {
        HStack(spacing: 6) {
            VStack(spacing: 10) {
                StatusDot()
                ThermalBadge(compact: true)
                FormatChip(expanded: $showOptions, compact: true)
                Spacer(minLength: 0)
                if needsPairing {
                    CircleButton(systemImage: "qrcode.viewfinder", label: "Pair with a computer", action: openScanner)
                }
                CircleButton(systemImage: "gearshape.fill", label: "Settings") {
                    showSettings = true
                }
            }
            .frame(width: 44)
            if showOptions {
                ScrollView {
                    OptionsPanel()
                }
                .frame(width: 300)
                .transition(.move(edge: .leading).combined(with: .opacity))
            }
            VideoFrame()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            LensPicker(vertical: true)
        }
    }
}

/// The camera preview in a rounded frame shaped like the picture itself, with the
/// flashlight / mirror / sleep buttons along its right edge.
private struct VideoFrame: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 20, style: .continuous)
        CameraPreview(layer: model.camera.previewLayer)
            .aspectRatio(model.previewAspect, contentMode: .fit)
            .clipShape(shape)
            .overlay(shape.strokeBorder(.white.opacity(0.18), lineWidth: 1))
            .overlay(alignment: .bottomTrailing) {
                ToggleColumn().padding(10)
            }
            .animation(.easeInOut(duration: 0.25), value: model.previewAspect)
    }
}

/// Black screen while streaming, to save battery and heat. Tap to wake.
private struct DimmedOverlay: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            VStack(spacing: 8) {
                Image(systemName: "video.fill")
                    .font(.title2)
                    .foregroundStyle(.green)
                Text(model.connectedTo.map { "Streaming to \($0)" } ?? "Waiting for computer")
                    .font(.footnote)
                if model.micOn {
                    Label("Microphone on", systemImage: "mic.fill")
                        .font(.caption2)
                }
                Text("Tap to wake")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .foregroundStyle(.white.opacity(0.5))
        }
        .contentShape(Rectangle())
        .onTapGesture { model.dimmed = false }
        .statusBarHidden()
        .persistentSystemOverlays(.hidden)
    }
}
