import SwiftUI

struct MenuBarView: View {
    @EnvironmentObject private var controller: JotbayController
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if controller.needsSetup {
                // Every row below assumes a vault. Offering Sync Now with
                // nothing to sync just produces an error the user cannot act on
                // from here — send them to the one screen that can fix it.
                unconfigured
            } else {
                header
                Divider()
                nodeSummary
            }
            Divider()
            actions
        }
        .frame(width: 300)
    }

    private var unconfigured: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                StatusDot(health: .diverged)
                Text("Not set up yet")
                    .font(.system(size: 13, weight: .semibold))
                Spacer()
            }
            Text("Jotbay needs to know where your notes live.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                StatusDot(health: overallHealth)
                Text(headline)
                    .font(.system(size: 13, weight: .semibold))
                Spacer()
                if controller.isSyncing {
                    ProgressView().controlSize(.small)
                }
            }

            if !controller.lastMessage.isEmpty {
                Text(controller.lastMessage)
                    .font(.system(size: 11))
                    .foregroundStyle(controller.lastMessageIsError ? Color.red : .secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    private var nodeSummary: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(controller.status.nodes.prefix(5)) { node in
                HStack(spacing: 8) {
                    StatusDot(health: node.health(localHead: controller.status.head))
                    Text(node.hostname)
                        .font(.system(size: 12))
                        .lineLimit(1)
                    Spacer()
                    Text(node.lastSync, format: .relative(presentation: .numeric))
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
            }

            if controller.status.nodes.isEmpty {
                Text("No machines have reported in yet")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var actions: some View {
        VStack(spacing: 0) {
            if controller.needsSetup {
                MenuButton(title: "Set Up Jotbay…", symbol: "sparkles", key: "m") {
                    openWindow(id: "main")
                    NSApp.activate(ignoringOtherApps: true)
                }
            } else {
                MenuButton(title: "Sync now", symbol: "arrow.triangle.2.circlepath", key: "s") {
                    controller.sync()
                }
                .disabled(controller.isSyncing)

                MenuButton(title: "Open Jotbay Folder", symbol: "folder", key: "o") {
                    controller.revealDataDirectory()
                }

                MenuButton(title: "Management Window", symbol: "square.grid.2x2", key: "m") {
                    openWindow(id: "main")
                    NSApp.activate(ignoringOtherApps: true)
                }

                MenuButton(title: "Terminal Dashboard", symbol: "terminal", key: "d") {
                    controller.openTerminalDashboard()
                }
            }

            Divider().padding(.vertical, 4)

            MenuButton(title: "Quit", symbol: "power", key: "q") {
                NSApp.terminate(nil)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
    }

    private var overallHealth: NodeHealth {
        if controller.binaryMissing || controller.status.rebaseInProgress { return .error }
        if controller.status.warnings.contains(where: { $0.severity == .blocked }) { return .error }
        if controller.status.nodes.contains(where: { $0.lastError != nil }) { return .error }
        if !controller.status.isClean { return .diverged }
        return .healthy
    }

    private var headline: String {
        if controller.binaryMissing { return "CLI not installed" }
        if controller.status.rebaseInProgress { return "Conflicts need attention" }

        // A blocked file outranks "in sync", because from here Jotbay would
        // otherwise look perfectly healthy while a file silently is not syncing.
        let blocked = controller.status.warnings.filter { $0.severity == .blocked }
        if !blocked.isEmpty {
            return "\(blocked.count) file\(blocked.count == 1 ? "" : "s") too large to sync"
        }

        if controller.status.isClean { return "This machine is in sync" }

        var parts: [String] = []
        if !controller.status.dirtyFiles.isEmpty {
            parts.append("\(controller.status.dirtyFiles.count) uncommitted")
        }
        if controller.status.ahead > 0 { parts.append("\(controller.status.ahead) to push") }
        if controller.status.behind > 0 { parts.append("\(controller.status.behind) to pull") }
        return parts.joined(separator: ", ")
    }
}

/// A menu row that highlights on hover, which a plain SwiftUI Button in a
/// MenuBarExtra window does not do on its own.
private struct MenuButton: View {
    let title: String
    let symbol: String
    let key: String
    let action: () -> Void

    @State private var hovering = false
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: symbol)
                    .frame(width: 16)
                    .foregroundStyle(hovering && isEnabled ? Color.white : .secondary)
                Text(title)
                    .font(.system(size: 13))
                    .foregroundStyle(hovering && isEnabled ? Color.white : .primary)
                Spacer()
                Text("⌘\(key.uppercased())")
                    .font(.system(size: 11, design: .rounded))
                    .foregroundStyle(hovering && isEnabled
                        ? Color.white.opacity(0.7)
                        : Color.secondary.opacity(0.6))
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 5)
                    .fill(hovering && isEnabled ? Color.accentColor : .clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(KeyEquivalent(Character(key)), modifiers: .command)
        .onHover { hovering = $0 }
        .opacity(isEnabled ? 1 : 0.45)
    }
}

struct StatusDot: View {
    let health: NodeHealth

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 8, height: 8)
            .overlay(Circle().stroke(color.opacity(0.35), lineWidth: 3).scaleEffect(1.6))
            .help(health.label)
    }

    private var color: Color {
        switch health {
        case .healthy: return .green
        // Blue, not amber: being behind resolves itself on that machine's next
        // pull, so it is information rather than something to act on. Amber is
        // reserved for genuine divergence, which does need a sync.
        case .behind: return .cyan
        case .diverged: return .orange
        case .stale: return .secondary
        case .error: return .red
        }
    }
}
