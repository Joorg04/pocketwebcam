import SwiftUI

@main
struct PocketWebcamApp: App {
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
                .task { model.start() }
                // A pairing code scanned with the iPhone's own Camera app opens this link.
                .onOpenURL { url in model.handlePairingLink(url.absoluteString) }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.becameActive() }
        }
    }
}
