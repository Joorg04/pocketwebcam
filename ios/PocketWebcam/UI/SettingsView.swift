import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    /// Opens the QR scanner (from the main screen, so Settings closes first).
    var onPair: () -> Void = {}

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $model.deviceName)
                        .textInputAutocapitalization(.words)
                } header: {
                    Text("Name on the network")
                } footer: {
                    Text("This is how your phone appears in Pocket Webcam on your computer.")
                }

                Section {
                    ForEach(model.pairedComputers) { computer in
                        Text(computer.name)
                    }
                    .onDelete { rows in
                        let gone = rows.map { model.pairedComputers[$0].fingerprint }
                        gone.forEach(model.forgetComputer)
                    }
                    Button {
                        onPair()
                    } label: {
                        Label("Pair with a computer", systemImage: "qrcode.viewfinder")
                    }
                } header: {
                    Text("Paired computers")
                } footer: {
                    Text("Only paired computers can connect to this phone, and everything they send each other is encrypted. Swipe left on a computer to forget it.")
                }

                Section("Video") {
                    Picker("Codec", selection: Binding(get: { model.config.codec }, set: model.setCodec)) {
                        Text("H.264 (most compatible)").tag("h264")
                        Text("HEVC (sharper, less bandwidth)").tag("hevc")
                    }
                    LabeledContent("Bitrate") {
                        Stepper(
                            "\(model.config.bitrate / 1_000_000) Mbps",
                            value: Binding(
                                get: { model.config.bitrate / 1_000_000 },
                                set: { model.apply(ConfigUpdate(bitrate: $0 * 1_000_000)) }
                            ),
                            in: 1...50
                        )
                        .fixedSize()
                    }
                    Toggle("Stabilization", isOn: Binding(get: { model.config.stabilization }, set: model.setStabilization))
                    if let cam = model.currentCamera, cam.zoom.max > cam.zoom.min {
                        VStack(alignment: .leading) {
                            Text("Zoom \(model.config.zoom, specifier: "%.1f")×")
                            Slider(
                                value: Binding(get: { model.config.zoom }, set: model.setZoom),
                                in: cam.zoom.min...cam.zoom.max
                            )
                        }
                    }
                }

                Section {
                    LabeledContent("Status", value: model.connectedTo.map { "Connected to \($0)" } ?? "Waiting")
                    if let problem = model.identityError {
                        Text(problem).foregroundStyle(.red)
                    }
                    if let ip = NetworkInfo.wifiAddress {
                        LabeledContent("Address", value: "\(ip):\(model.listeningPort.map(String.init) ?? "–")")
                            .textSelection(.enabled)
                    }
                } header: {
                    Text("Connection")
                } footer: {
                    Text("If your computer can't find the phone automatically, use Connect by IP address in Pocket Webcam on the computer and type this address.")
                }

                Section("About") {
                    Link("Get Pocket Webcam for your computer", destination: URL(string: "https://pocketwebcam.com/download")!)
                    Link("Privacy policy", destination: URL(string: "https://pocketwebcam.com/privacy")!)
                    LabeledContent("Version", value: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "–")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}
