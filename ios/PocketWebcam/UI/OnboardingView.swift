import SwiftUI

/// First-launch explainer. App Review needs to understand the app requires a companion
/// desktop program, so this says so plainly and links to it.
struct OnboardingView: View {
    let onDone: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 28) {
            Spacer()
            VStack(alignment: .leading, spacing: 8) {
                Image(systemName: "web.camera.fill")
                    .font(.system(size: 44))
                    .foregroundStyle(.yellow)
                Text("Your iPhone is now\na webcam.")
                    .font(.largeTitle.bold())
                Text("Free, over Wi-Fi, at full resolution.")
                    .font(.title3)
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 20) {
                Step(number: 1, title: "Install Pocket Webcam on your computer",
                     detail: "Download it from pocketwebcam.com.")
                Step(number: 2, title: "Pair them once",
                     detail: "On the same Wi-Fi, tap Pair with a computer and scan the code Pocket Webcam shows on the computer.")
                Step(number: 3, title: "Keep this app open",
                     detail: "Your computer connects by itself. Pick Pocket Webcam as the camera in Zoom, Meet, OBS or any other app.")
            }

            Spacer()

            VStack(spacing: 12) {
                Link("Get the computer app", destination: URL(string: "https://pocketwebcam.com/download")!)
                    .font(.headline)
                    .frame(maxWidth: .infinity)
                Button(action: onDone) {
                    Text("Continue")
                        .font(.headline)
                        .foregroundStyle(.black)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 16)
                        .background(.yellow, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                }
            }
        }
        .padding(28)
        .background(Color.black.ignoresSafeArea())
    }
}

private struct Step: View {
    let number: Int
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Text("\(number)")
                .font(.headline.monospacedDigit())
                .foregroundStyle(.black)
                .frame(width: 30, height: 30)
                .background(Circle().fill(.white))
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.headline)
                Text(detail).font(.subheadline).foregroundStyle(.secondary)
            }
        }
    }
}
