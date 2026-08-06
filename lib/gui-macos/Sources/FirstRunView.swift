import SwiftUI

/// Shown when there is no vault on this machine.
///
/// An installed app always opens here the first time: a `.dmg` cannot clone a
/// private repository, so somebody has to be asked where the notes live. Three
/// routes, with the one requiring no git knowledge offered first, and disabled
/// rather than allowed to fail when `gh` cannot actually deliver it.
struct FirstRunView: View {
    @EnvironmentObject private var controller: JotbayController

    @State private var mode: SetupMode?
    @State private var value = ""
    @State private var location = ""

    var body: some View {
        GeometryReader { geo in
            ScrollView {
                content
                    .frame(maxWidth: 520, alignment: .leading)
                    .padding(.horizontal, 36)
                    .padding(.vertical, 40)
                    // Centred while it fits, scrolling once it does not. A
                    // top-aligned welcome screen leaves a third of the window
                    // empty, which reads as something failing to load.
                    .frame(maxWidth: .infinity, minHeight: geo.size.height)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { adoptDefaultLocation() }
        .onChange(of: controller.capabilities?.defaultLocation) { _, _ in
            adoptDefaultLocation()
        }
    }

    private var caps: SetupCapabilities? { controller.capabilities }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            AppIcon(size: 64)
                .padding(.bottom, 14)

            Text("Welcome to Jotbay")
                .font(.system(size: 22, weight: .semibold))
            Text("Your notes sync between machines through a private git repository. Choose how to set this one up.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)

            VStack(spacing: 8) {
                ForEach(SetupMode.allCases, id: \.self) { option in
                    OptionCard(
                        title: option.title,
                        detail: detail(for: option),
                        enabled: enabled(option),
                        selected: mode == option
                    ) { select(option) }
                }
            }
            .padding(.top, 20)

            if let mode {
                SetupForm(
                    mode: mode,
                    value: $value,
                    location: $location,
                    busy: controller.setupBusy,
                    browse: {
                        if let picked = controller.chooseFolder(startingAt: location) {
                            location = picked
                        }
                    },
                    back: { self.mode = nil },
                    submit: { controller.runSetup(mode: mode.rawValue, value: value, location: location) }
                )
                .padding(.top, 18)
            }

            if let error = controller.setupError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 14)
            }

            if caps?.git == false {
                Label("git is not installed on this machine. Install it, then reopen Jotbay.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 14)
            }

            // The whole app is usable without ever opening this window, and
            // someone who would rather not should be told so rather than left
            // to discover it.
            HStack(spacing: 4) {
                Text("Prefer a terminal?")
                Text("jotbay init")
                    .font(.system(size: 11.5, design: .monospaced))
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(RoundedRectangle(cornerRadius: 4).fill(.quaternary.opacity(0.6)))
                Text("does the same thing.")
            }
            .font(.system(size: 11.5))
            .foregroundStyle(.tertiary)
            .padding(.top, 24)
        }
    }

    // MARK: - Option state

    private func enabled(_ option: SetupMode) -> Bool {
        guard caps?.git == true else { return false }
        return option == .create ? caps?.ghAuthenticated == true : true
    }

    /// The `create` row explains *why* it is unavailable rather than simply
    /// greying out, because the remedy is one command and nothing else on this
    /// screen can hint at it.
    private func detail(for option: SetupMode) -> String {
        guard option == .create else { return option.detail }
        guard let caps else { return "Checking" }
        if caps.ghAuthenticated {
            return caps.login.map { "Makes a new private repository under \($0) and starts syncing." }
                ?? option.detail
        }
        return caps.gh
            ? "Sign in first: run gh auth login in a terminal, then reopen Jotbay."
            : "Needs the GitHub CLI (gh) installed and signed in."
    }

    private func select(_ option: SetupMode) {
        mode = option
        controller.setupError = nil
        value = option == .create ? "jotbay-notes" : ""
    }

    private func adoptDefaultLocation() {
        if location.isEmpty { location = caps?.defaultLocation ?? "" }
    }
}

enum SetupMode: String, CaseIterable {
    case create, clone, adopt

    var title: String {
        switch self {
        case .create: return "Create one for me"
        case .clone:  return "I already have one"
        case .adopt:  return "Use a folder on this machine"
        }
    }

    var detail: String {
        switch self {
        case .create: return "Makes a new private repository on GitHub and starts syncing."
        case .clone:  return "Clone a repository you or another machine already set up."
        case .adopt:  return "Point Jotbay at a clone that is already here."
        }
    }
}

// MARK: - Form

struct SetupForm: View {
    let mode: SetupMode
    @Binding var value: String
    @Binding var location: String
    let busy: Bool
    let browse: () -> Void
    let back: () -> Void
    let submit: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if mode != .adopt {
                FieldLabel(mode == .create ? "Repository name" : "Repository URL")
                TextField(
                    mode == .create ? "jotbay-notes" : "https://github.com/you/your-notes.git",
                    text: $value
                )
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12.5))
            }

            FieldLabel(mode == .adopt ? "Existing folder" : "Location")
            HStack(spacing: 8) {
                TextField("", text: $location)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12.5))
                // Typing a filesystem path is exactly what someone who opened
                // the app instead of a terminal is trying to avoid.
                Button("Choose", action: browse)
            }

            HStack(spacing: 10) {
                if busy {
                    ProgressView().controlSize(.small)
                    Text(mode == .adopt ? "Checking the folder" : "This can take a moment")
                        .font(.system(size: 11.5))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Back", action: back)
                    .disabled(busy)
                Button(busy ? "Setting up" : "Set up Jotbay", action: submit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || location.isEmpty || (mode != .adopt && value.isEmpty))
            }
            .padding(.top, 4)
        }
    }
}

private struct FieldLabel: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundStyle(.secondary)
            .textCase(.uppercase)
    }
}

// MARK: - Option card

struct OptionCard: View {
    let title: String
    let detail: String
    let enabled: Bool
    let selected: Bool
    let action: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(enabled ? .primary : .tertiary)
                Text(detail)
                    .font(.system(size: 11.5))
                    .foregroundStyle(enabled ? .secondary : .tertiary)
                    .fixedSize(horizontal: false, vertical: true)
                    .multilineTextAlignment(.leading)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 15)
            .padding(.vertical, 13)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(fill)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(selected ? Color.accentColor : Color.secondary.opacity(0.25),
                            lineWidth: selected ? 1.5 : 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .onHover { hovering = $0 && enabled }
    }

    private var fill: Color {
        if selected { return Color.accentColor.opacity(0.12) }
        return hovering ? Color.secondary.opacity(0.08) : .clear
    }
}
