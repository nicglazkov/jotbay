import SwiftUI

/// The one screen between finishing setup and using the app.
///
/// It exists to offer the two shortcuts, which an installer cannot: at install
/// time the app has not been told where the notes go, so a "create a shortcut
/// to your notes folder" checkbox in a `.pkg` would have to guess. Here both
/// targets exist and can be named.
struct SetupDoneView: View {
    @EnvironmentObject private var controller: JotbayController

    @State private var wantsNotes = true
    @State private var wantsApp = true

    var body: some View {
        GeometryReader { geo in
            ScrollView {
                content
                    .frame(maxWidth: 520, alignment: .leading)
                    .padding(.horizontal, 36)
                    .padding(.vertical, 40)
                    .frame(maxWidth: .infinity, minHeight: geo.size.height)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var appAvailable: Bool { controller.capabilities?.appInstalled ?? false }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            AppIcon(size: 64)
                .padding(.bottom, 14)

            Text("Jotbay is ready")
                .font(.system(size: 22, weight: .semibold))

            VStack(alignment: .leading, spacing: 4) {
                Text("Your notes live in")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                Text(controller.dataDirectory.path)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(RoundedRectangle(cornerRadius: 5).fill(.quaternary.opacity(0.5)))
                Text("Anything you put there reaches your other machines on the next sync.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 8)

            VStack(alignment: .leading, spacing: 10) {
                Toggle("Put a shortcut to my notes on the desktop", isOn: $wantsNotes)
                // An inert checkbox is worse than an absent one.
                if appAvailable {
                    Toggle("Put a shortcut to Jotbay on the desktop", isOn: $wantsApp)
                }
            }
            .font(.system(size: 13))
            .padding(.top, 22)

            if let desktop = controller.capabilities?.desktop {
                Text("Shortcuts go in \(desktop)")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .padding(.top, 8)
            }

            HStack {
                Spacer()
                Button("Continue") {
                    controller.finishSetup(app: wantsApp && appAvailable, notes: wantsNotes)
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 22)

            HStack(spacing: 4) {
                Text("You can do this later with")
                Text("jotbay shortcut")
                    .font(.system(size: 11.5, design: .monospaced))
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(RoundedRectangle(cornerRadius: 4).fill(.quaternary.opacity(0.6)))
            }
            .font(.system(size: 11.5))
            .foregroundStyle(.tertiary)
            .padding(.top, 20)
        }
    }
}
