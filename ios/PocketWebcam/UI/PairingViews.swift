import SwiftUI
import VisionKit

/// Scans the QR code that Pocket Webcam shows on the computer.
struct PairScannerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var notOurs = false

    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
                    QRScanner { link in
                        if model.handlePairingLink(link) {
                            dismiss()
                        } else {
                            notOurs = true
                        }
                    }
                    .ignoresSafeArea()
                    .overlay(alignment: .bottom) {
                        Text(notOurs ? "That isn't a Pocket Webcam pairing code." : "Point the camera at the code on your computer.")
                            .font(.callout)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 10)
                            .background(.black.opacity(0.6), in: Capsule())
                            .padding(.bottom, 32)
                    }
                } else {
                    ContentUnavailableView(
                        "The scanner isn't available",
                        systemImage: "qrcode.viewfinder",
                        description: Text("Scan the code on your computer with the iPhone's Camera app instead, then tap the link it shows.")
                    )
                }
            }
            .navigationTitle("Pair with a computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }
}

/// VisionKit's scanner, for QR codes only. Calls `onCode` with each new code's text.
private struct QRScanner: UIViewControllerRepresentable {
    var onCode: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ scanner: DataScannerViewController, coordinator: Coordinator) {
        scanner.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    @MainActor
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onCode: (String) -> Void
        private var handled = false

        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                guard case .barcode(let code) = item, let text = code.payloadStringValue else { continue }
                if text.hasPrefix("pocketwebcam://pair") {
                    guard !handled else { return }
                    handled = true
                }
                onCode(text)
            }
        }
    }
}

/// Pairing as it happens: waiting for the computer, the code to compare, the outcome.
/// An overlay card rather than a sheet, so it can come up over anything, any time.
struct PairingCard: View {
    @EnvironmentObject private var model: AppModel
    let prompt: PairingPrompt

    var body: some View {
        VStack(spacing: 18) {
            switch prompt {
            case .waiting(let computer):
                ProgressView().controlSize(.large)
                Text("Pairing with \(computer)…").font(.title3.weight(.semibold))
                Text("Keep Pocket Webcam open on the computer. Both need to be on the same Wi-Fi.")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                Button("Cancel", role: .cancel) { model.cancelPairing() }
                    .buttonStyle(.bordered)
            case .code(let computer, let code), .confirmed(let computer, let code):
                Text("Pair with \(computer)?").font(.title3.weight(.semibold))
                Text("Check that the computer shows the same code:")
                    .foregroundStyle(.secondary)
                Text(Pairing.spaced(code))
                    .font(.system(size: 44, weight: .bold, design: .monospaced))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
                if case .confirmed = prompt {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("Now confirm on the computer.").foregroundStyle(.secondary)
                    }
                    Button("Cancel", role: .cancel) { model.cancelPairing() }
                        .buttonStyle(.bordered)
                } else {
                    Text("If the codes are different, don't pair: something else is answering.")
                        .font(.footnote)
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 12) {
                        Button("Cancel", role: .cancel) { model.answerCode(false) }
                            .buttonStyle(.bordered)
                        Button("Pair") { model.answerCode(true) }
                            .buttonStyle(.borderedProminent)
                    }
                }
            case .paired(let computer):
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 48))
                    .foregroundStyle(.green)
                Text("Paired with \(computer)").font(.title3.weight(.semibold))
                Text("From now on they connect by themselves whenever the app is open.")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                Button("Done") { model.pairing = nil }
                    .buttonStyle(.borderedProminent)
            case .failed(let why):
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 44))
                    .foregroundStyle(.orange)
                Text("Not paired").font(.title3.weight(.semibold))
                Text(why)
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                Button("OK") { model.pairing = nil }
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(24)
        .frame(maxWidth: 420)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.black.opacity(0.55).ignoresSafeArea())
        .accessibilityAddTraits(.isModal)
    }
}
