import SwiftUI

/// Chooses between first-run setup and the app proper.
///
/// A `.dmg` cannot carry a private repository, so an installed app's very first
/// launch has no vault to show. Deciding here rather than inside `MainWindow`
/// keeps the setup screen free of a toolbar whose every button would be dead.
struct RootView: View {
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        if !controller.setupChecked {
            // Neither view is right yet, and guessing produces a visible swap.
            ProgressView()
                .controlSize(.small)
                .frame(minWidth: 760, minHeight: 480)
        } else if controller.needsSetup {
            FirstRunView()
                .frame(minWidth: 620, minHeight: 480)
        } else if controller.justSetUp {
            SetupDoneView()
                .frame(minWidth: 620, minHeight: 480)
        } else {
            MainWindow()
        }
    }
}

struct MainWindow: View {
    @EnvironmentObject private var controller: JotbayController
    @State private var showSettings = false

    var body: some View {
        VStack(spacing: 0) {
            SummaryBar()
            Divider()
            HStack(spacing: 0) {
                // Machines holds a handful of rows; Activity holds dozens.
                // The old split gave the larger half to the emptier pane.
                NodesPane()
                    .frame(minWidth: 260, idealWidth: 300, maxWidth: 380)
                Divider()
                ActivityPane()
                    .frame(minWidth: 380, maxWidth: .infinity)
            }
        }
        .frame(minWidth: 760, minHeight: 480)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    controller.sync()
                } label: {
                    Label("Sync", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(controller.isSyncing)
                .help("Commit, integrate the remote, and push")
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    controller.refresh(fetchRemote: true)
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help("Fetch the latest state from the remote")
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    controller.revealDataDirectory()
                } label: {
                    Label("Open Folder", systemImage: "folder")
                }
                .help("Reveal the synced data directory in Finder")
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showSettings.toggle()
                } label: {
                    Label("Settings", systemImage: "gearshape")
                }
                .help("Appearance and advanced options")
                .popover(isPresented: $showSettings, arrowEdge: .bottom) {
                    SettingsPanel().environmentObject(controller)
                }
            }
        }
    }
}

// MARK: - Summary

private struct SummaryBar: View {
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        HStack(spacing: 20) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    StatusDot(health: health)
                    Text(headline)
                        .font(.system(size: 15, weight: .semibold))
                    if controller.isSyncing {
                        ProgressView().controlSize(.small)
                    }
                }
                Text(controller.status.root.isEmpty ? "Locating Jotbay…" : controller.status.root)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            Spacer()

            Metric(value: "\(controller.status.dataFiles)", label: "files")
            Metric(value: "\(controller.status.nodes.count)", label: "machines")
            Metric(value: controller.status.headShort.isEmpty ? "—" : controller.status.headShort,
                   label: "commit")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
        .background(.bar)
    }

    private var health: NodeHealth {
        if controller.binaryMissing || controller.status.rebaseInProgress { return .error }
        if controller.status.nodes.contains(where: { $0.lastError != nil }) { return .error }
        return controller.status.isClean ? .healthy : .diverged
    }

    private var headline: String {
        if controller.binaryMissing { return "Command line tool not installed" }
        if controller.status.rebaseInProgress {
            return "\(controller.status.conflicts.count) file(s) need resolving"
        }
        // The dot reflects the whole mesh while this only described the local
        // machine, so the two contradicted each other: a red dot beside "This
        // machine is in sync", with a machine that had not synced in six hours
        // listed directly underneath.
        let failing = controller.status.nodes.filter { $0.lastError != nil }.count

        var local = "This machine is in sync"
        if !controller.status.isClean {
            var parts: [String] = []
            if !controller.status.dirtyFiles.isEmpty {
                parts.append("\(controller.status.dirtyFiles.count) uncommitted")
            }
            if controller.status.ahead > 0 { parts.append("\(controller.status.ahead) to push") }
            if controller.status.behind > 0 { parts.append("\(controller.status.behind) to pull") }
            local = parts.joined(separator: ", ").capitalizedFirst
        }

        guard failing > 0 else { return local }
        return failing == 1
            ? "\(local) · 1 machine needs attention"
            : "\(local) · \(failing) machines need attention"
    }
}

private struct Metric: View {
    let value: String
    let label: String

    var body: some View {
        VStack(alignment: .trailing, spacing: 2) {
            Text(value)
                .font(.system(size: 16, weight: .medium, design: .rounded))
                .monospacedDigit()
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .textCase(.uppercase)
        }
    }
}

// MARK: - Nodes

private struct NodesPane: View {
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PaneHeader(title: "Machines", count: controller.status.nodes.count)

            if controller.status.nodes.isEmpty {
                EmptyPane(
                    symbol: "desktopcomputer.trianglebadge.exclamationmark",
                    title: "No machines yet",
                    detail: "Each machine reports in the first time it syncs."
                )
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(controller.status.nodes) { node in
                            NodeRow(node: node, localHead: controller.status.head)
                            Divider().padding(.leading, 20)
                        }
                    }
                }
            }

            if controller.status.rebaseInProgress {
                ConflictBanner(files: controller.status.conflicts)
            }
            if let latest = controller.status.updateAvailable {
                UpdateBanner(version: latest)
            }
            if !controller.status.warnings.isEmpty {
                FileLimitBanner(warnings: controller.status.warnings)
            }
        }
    }
}

/// Files that will not sync, or that will cost more than expected.
///
/// Blocked files are the important case: they are sitting in your notes folder looking
/// synced, and without this the only sign would be their absence elsewhere.
private struct FileLimitBanner: View {
    let warnings: [FileWarning]

    private var blocked: [FileWarning] { warnings.filter { $0.severity == .blocked } }
    private var large: [FileWarning] { warnings.filter { $0.severity != .blocked } }

    /// Whatever `jotbay_core::limits` says about these files. Holding a second
    /// copy of the wording here is what left this window recommending Git LFS
    /// after core had stopped — and LFS corrupts files during conflict
    /// resolution. The fallback only covers an `jotbay` binary older than the
    /// field itself.
    private func advice(for group: [FileWarning]) -> String {
        group.compactMap(\.advice).first ?? "Some files could not be synced as-is."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !blocked.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Label(
                        "\(blocked.count) file\(blocked.count == 1 ? "" : "s") can't be synced",
                        systemImage: "xmark.octagon.fill"
                    )
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.red)

                    ForEach(blocked) { w in
                        HStack(spacing: 6) {
                            Text(w.filename).font(.system(size: 11, weight: .medium))
                            Text(w.humanSize)
                                .font(.system(size: 11, design: .rounded))
                                .foregroundStyle(.red)
                            Spacer()
                        }
                    }

                    Text(advice(for: blocked))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if !large.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Label(
                        "\(large.count) large file\(large.count == 1 ? "" : "s")",
                        systemImage: "exclamationmark.triangle.fill"
                    )
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.orange)

                    ForEach(large) { w in
                        HStack(spacing: 6) {
                            Text(w.filename).font(.system(size: 11))
                            Text(w.humanSize)
                                .font(.system(size: 11, design: .rounded))
                                .foregroundStyle(.secondary)
                            Spacer()
                        }
                    }

                    Text(advice(for: large))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(blocked.isEmpty ? Color.orange.opacity(0.10) : Color.red.opacity(0.10))
    }
}

private struct NodeRow: View {
    let node: NodeStatus
    let localHead: String

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            StatusDot(health: health)
                .padding(.top, 5)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(node.hostname)
                        .font(.system(size: 13, weight: .medium))
                    Text(node.os)
                        .font(.system(size: 10))
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Capsule().fill(.quaternary))
                }

                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)

                if let error = node.lastError {
                    Text(error)
                        .font(.system(size: 11))
                        .foregroundStyle(.red)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Spacer()

            VStack(alignment: .trailing, spacing: 3) {
                Text(node.lastSync, format: .relative(presentation: .numeric))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Text(node.shortHead)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 11)
    }

    private var health: NodeHealth { node.health(localHead: localHead) }

    private var detail: String {
        // The health label used to be prepended unconditionally, which read
        // "behind · 1 behind" — the count already says it, and says it more
        // precisely. The label is only worth showing when nothing else is.
        var parts: [String] = []
        if node.behind > 0 { parts.append("\(node.behind) behind") }
        if node.ahead > 0 { parts.append("\(node.ahead) ahead") }
        if node.dirty > 0 { parts.append("\(node.dirty) uncommitted") }
        if node.conflictsResolved > 0 {
            parts.append("\(node.conflictsResolved) conflict\(node.conflictsResolved == 1 ? "" : "s") kept")
        }
        return parts.isEmpty ? health.label : parts.joined(separator: " · ")
    }
}

private struct ConflictBanner: View {
    let files: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label("\(files.count) file(s) awaiting resolution", systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 12, weight: .medium))
            Text("Running Sync keeps both versions automatically.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(Color.orange.opacity(0.12))
    }
}

// MARK: - Activity

private struct ActivityPane: View {
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PaneHeader(title: "Activity", count: controller.activity.count)

            if controller.activity.isEmpty {
                EmptyPane(
                    symbol: "clock",
                    title: "Nothing has happened yet",
                    detail: "Syncs that change nothing aren't recorded, so this stays quiet until something moves."
                )
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(controller.activity) { event in
                            EventRow(event: event, verbose: controller.settings.verbose)
                            Divider().padding(.leading, 20)
                        }
                    }
                }
            }
        }
    }
}

private struct EventRow: View {
    let event: ActivityEvent
    let verbose: Bool

    @State private var expanded = false

    private var files: [String] { event.files ?? [] }
    private var canExpand: Bool { !files.isEmpty || (verbose && event.detail != nil) }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: event.kind.symbol)
                    .font(.system(size: 11))
                    .foregroundStyle(tint)
                    .frame(width: 14)
                    .padding(.top, 2)

                VStack(alignment: .leading, spacing: 3) {
                    Text(event.summary)
                        .font(.system(size: 12))
                        .foregroundStyle(event.kind == .error ? Color.red : .primary)
                        .lineLimit(3)
                        .fixedSize(horizontal: false, vertical: true)

                    HStack(spacing: 6) {
                        Text(event.hostname)
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                        // The timestamp sits with the machine name rather than
                        // floated to the far edge, where a wide window stranded
                        // it from the row it describes.
                        Text(event.at, format: .relative(presentation: .numeric))
                            .font(.system(size: 10))
                            .foregroundStyle(.tertiary)
                        if canExpand {
                            Button(expanded ? "▾ hide" : "▸ details") {
                                expanded.toggle()
                            }
                            .buttonStyle(.plain)
                            .font(.system(size: 10))
                            .foregroundStyle(.tertiary)
                        }
                        Spacer()
                    }
                }
            }

            if expanded {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(files, id: \.self) { f in
                        Text(f)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                    }
                    if verbose, let detail = event.detail {
                        Text(detail)
                            .font(.system(size: 10.5, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                            .padding(8)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(RoundedRectangle(cornerRadius: 6).fill(.quaternary.opacity(0.4)))
                    }
                }
                .padding(.leading, 24)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 9)
    }

    private var tint: Color {
        switch event.kind {
        case .changed: return .accentColor
        case .conflict: return .orange
        case .error: return .red
        }
    }
}

// MARK: - Shared chrome

private struct PaneHeader: View {
    let title: String
    let count: Int

    var body: some View {
        HStack {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            Spacer()
            Text("\(count)")
                .font(.system(size: 11, design: .rounded))
                .foregroundStyle(.tertiary)
                .monospacedDigit()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 9)
        .background(.quaternary.opacity(0.35))
    }
}

private struct EmptyPane: View {
    let symbol: String
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 26))
                .foregroundStyle(.quaternary)
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            Text(detail)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(30)
    }
}


/// Per-machine preferences. Deliberately not synced: a laptop following the
/// system theme and a desktop pinned to dark are both correct at once.
private struct SettingsPanel: View {
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Appearance")
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .textCase(.uppercase)
                Picker("", selection: Binding(
                    get: { controller.settings.theme },
                    set: { controller.updateSettings(theme: $0) }
                )) {
                    Text("Match system").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                Text("Applies to this machine only — settings never sync.")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }

            Divider()

            VStack(alignment: .leading, spacing: 5) {
                Text("Advanced")
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .textCase(.uppercase)
                Toggle("Verbose activity", isOn: Binding(
                    get: { controller.settings.verbose },
                    set: { controller.updateSettings(verbose: $0) }
                ))
                .font(.system(size: 12))
                Text("Show the raw underlying detail, including full git output on failures.")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(16)
        .frame(width: 320)
    }
}


/// Offered rather than applied. The repository already carries the marker that
/// says a release exists, so noticing costs nothing; installing stays a choice.
private struct UpdateBanner: View {
    let version: String
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "arrow.down.circle.fill")
                .foregroundStyle(Color.accentColor)
            Text("Version \(version) is available.")
                .font(.system(size: 12))
            Button("Update now") { controller.upgrade() }
                .buttonStyle(.link)
                .font(.system(size: 12))
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color.accentColor.opacity(0.10))
    }
}
