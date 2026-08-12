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
                RightPane()
                    .frame(minWidth: 380, maxWidth: .infinity)
            }
        }
        .frame(minWidth: 760, minHeight: 480)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    controller.sync()
                } label: {
                    Label("Sync now", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(controller.isSyncing)
                // One button. Refresh used to sit beside it, which asked the
                // user to know the difference between fetching what others did
                // and sending what they did. A git distinction, in an app
                // whose point is not needing one.
                .help("Send your changes and bring in everyone else's")
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
                Text(controller.status.root.isEmpty ? "Locating Jotbay" : controller.status.root)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            Spacer()

            Metric(value: "\(controller.status.dataFiles)", label: "files")
            Metric(value: "\(controller.status.nodes.count)", label: "machines")
            Metric(value: controller.status.headShort.isEmpty ? "-" : controller.status.headShort,
                   label: "commit")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
        .background(.bar)
    }

    private var health: NodeHealth {
        if controller.binaryMissing || controller.status.rebaseInProgress { return .error }
        // Asked of `health`, not of `lastError`. A machine that cannot reach
        // the network also sets lastError, and testing that directly turned
        // every commute into a red dot for the whole fleet.
        if failingNodes > 0 { return .error }
        return controller.status.isClean ? .healthy : .diverged
    }

    private var failingNodes: Int {
        controller.status.nodes.filter {
            $0.health(localHead: controller.status.head) == .error
        }.count
    }

    private var offlineNodes: Int {
        controller.status.nodes.filter {
            $0.health(localHead: controller.status.head) == .offline
        }.count
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
        let failing = failingNodes

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

        if failing > 0 {
            return failing == 1
                ? "\(local) · 1 machine needs attention"
                : "\(local) · \(failing) machines need attention"
        }
        // Mentioned, never as a warning. A machine that is merely off the
        // network needs nothing from anyone, and it comes back on its own.
        if offlineNodes > 0 {
            return offlineNodes == 1
                ? "\(local) · 1 machine offline"
                : "\(local) · \(offlineNodes) machines offline"
        }
        return local
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

            // Every message this window produced went to `lastMessage`, which
            // only the menu bar popover rendered. From the window itself,
            // pressing "Update now" or hitting a sync failure looked like
            // pressing a dead button: the app had plenty to say and nowhere
            // to say it.
            if !controller.lastMessage.isEmpty {
                MessageBar(
                    text: controller.lastMessage,
                    isError: controller.lastMessageIsError
                ) { controller.lastMessage = "" }
            }
            if controller.status.rebaseInProgress {
                ConflictBanner(files: controller.status.conflicts)
            }
            // Ahead of the update banner on purpose. If this process is stale,
            // "a new version is available" is the wrong advice: the new
            // version is already installed, and only a relaunch reaches it.
            if let installed = controller.replacedOnDisk {
                RestartBanner(version: installed)
            } else if let latest = controller.status.updateAvailable {
                UpdateBanner(version: latest, about: controller.about)
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
    /// after core had stopped, and LFS corrupts files during conflict
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
        // "behind · 1 behind". The count already says it, and says it more
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

// MARK: - Right pane

/// One pane, two lenses: what happened, and what is there.
private struct RightPane: View {
    @EnvironmentObject private var controller: JotbayController
    // Files, not Activity. The notes are the reason the window is open; what
    // the machines did about them is the second question, and only
    // occasionally the interesting one.
    @State private var tab: Tab = .files

    /// Declaration order is display order.
    enum Tab: String, CaseIterable {
        case files = "Files"
        case activity = "Activity"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                // Same place, same weight as the other pane headers, so the
                // tabs read as part of the chrome rather than as content.
                HStack(spacing: 2) {
                    ForEach(Tab.allCases, id: \.self) { candidate in
                        Button(candidate.rawValue) { tab = candidate }
                            .buttonStyle(.plain)
                            .font(.system(size: 11, weight: .semibold))
                            .textCase(.uppercase)
                            .foregroundStyle(tab == candidate ? .primary : .secondary)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(
                                RoundedRectangle(cornerRadius: 5)
                                    .fill(tab == candidate ? Color.primary.opacity(0.08) : .clear)
                            )
                    }
                }
                Spacer()
                if tab == .activity {
                    Text("\(controller.activity.count)")
                        .font(.system(size: 11, design: .rounded))
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 7)
            .background(.quaternary.opacity(0.35))

            switch tab {
            case .activity: ActivityPane()
            case .files: FilesPane()
            }
        }
    }
}

// MARK: - Activity

private struct ActivityPane: View {
    @EnvironmentObject private var controller: JotbayController

    private var raw: Bool { controller.settings.rawActivity }
    private var count: Int { raw ? controller.activity.count : controller.changes.count }
    private var isEmpty: Bool { raw ? controller.activity.isEmpty : controller.changes.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PaneHeader(title: raw ? "Machine activity" : "Activity", count: count)

            if isEmpty {
                EmptyPane(
                    symbol: "clock",
                    title: "Nothing has happened yet",
                    detail: "Syncs that change nothing aren't recorded, so this stays quiet until something moves."
                )
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        if raw {
                            ForEach(controller.activity) { event in
                                EventRow(event: event, verbose: controller.settings.verbose)
                                Divider().padding(.leading, 20)
                            }
                        } else {
                            ForEach(controller.changes) { change in
                                ChangeRow(change: change, verbose: controller.settings.verbose)
                                Divider().padding(.leading, 20)
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One change, however many machines reported it.
private struct ChangeRow: View {
    let change: Change
    let verbose: Bool

    @State private var expanded = false

    private var canExpand: Bool { !change.files.isEmpty || (verbose && change.detail != nil) }

    private var tint: Color {
        switch change.kind {
        case .updated: return .accentColor
        case .conflict: return .orange
        case .offline: return .secondary
        case .problem: return .red
        }
    }

    /// Who and where, without claiming authorship this machine cannot know.
    private var attribution: String {
        var parts: [String] = []
        if let origin = change.origin {
            parts.append(origin)
        } else if change.machines.count == 1 {
            parts.append(change.machines[0])
        }
        if change.machines.count > 1 {
            parts.append("on \(change.machines.count) machines")
        }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: change.kind.symbol)
                    .font(.system(size: 11))
                    .foregroundStyle(tint)
                    .frame(width: 14)
                    .padding(.top, 2)

                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(change.summary)
                            .font(.system(size: 12))
                            .foregroundStyle(change.kind == .problem ? Color.red : .primary)
                            .lineLimit(3)
                            .fixedSize(horizontal: false, vertical: true)
                        // Persistence, not a tally: it says this is still going.
                        if change.repeats > 1 {
                            Text("×\(change.repeats)")
                                .font(.system(size: 10, weight: .medium))
                                .foregroundStyle(.secondary)
                        }
                    }

                    HStack(spacing: 6) {
                        if !attribution.isEmpty {
                            Text(attribution)
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                        Text(change.at, format: .relative(presentation: .numeric))
                            .font(.system(size: 10))
                            .foregroundStyle(.tertiary)
                        if canExpand {
                            Button(expanded ? "▾ hide" : "▸ details") { expanded.toggle() }
                                .buttonStyle(.plain)
                                .font(.system(size: 10))
                                .foregroundStyle(.tertiary)
                        }
                    }

                    if expanded {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(change.files, id: \.self) { f in
                                Text(f)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(.secondary)
                            }
                            if verbose, let detail = change.detail {
                                Text(detail)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(.tertiary)
                                    .textSelection(.enabled)
                            }
                        }
                        .padding(.top, 2)
                    }
                }
                Spacer()
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 9)
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
        // Grey, like "stale". There is nothing to act on, and the next sync
        // after the network returns clears it.
        case .offline: return .secondary
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

struct EmptyPane: View {
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
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                about
                Divider()
                notes
                Divider()
                backgroundSync
                Divider()
                appearance
                Divider()
                advanced
            }
            .padding(16)
        }
        .frame(width: 380)
        .frame(maxHeight: 560)
        .onAppear { controller.loadAbout() }
    }

    // MARK: - Sections

    private var about: some View {
        Section("About") {
            Row("Version", controller.about?.version ?? "-")
            if let a = controller.about {
                Row("This machine", "\(a.hostname), \(a.os) \(a.arch)")
            }

            HStack(spacing: 8) {
                Button(controller.checkingUpdates ? "Checking" : "Check for updates") {
                    controller.checkForUpdates()
                }
                .disabled(controller.checkingUpdates)
                .font(.system(size: 12))
                if controller.checkingUpdates { ProgressView().controlSize(.small) }
            }
            .padding(.top, 2)

            // The update offer and the restart offer are different remedies
            // and must not be confused: one downloads, the other only relaunches.
            if let installed = controller.replacedOnDisk {
                Note("Version \(installed) is installed. This window is still running the old one.",
                     tone: .warning)
                Button("Restart Jotbay") { controller.restartIntoNewVersion() }
                    .font(.system(size: 12))
            } else if let latest = controller.about?.updateAvailable {
                Note("Version \(latest) is available.", tone: .warning)
                Button(controller.upgrading ? "Updating" : "Update now") {
                    controller.upgrade()
                }
                .font(.system(size: 12))
                .disabled(controller.upgrading)
            } else if let result = controller.updateCheckResult {
                Note(result, tone: .plain)
            }
        }
    }

    private var notes: some View {
        Section("Notes") {
            if let a = controller.about {
                Row("Folder", a.notes, mono: true)
                Row("Files", String(a.files))
                Row("Branch", a.branch)
                // Shown with any credentials removed by the engine, because a
                // settings panel is a thing people screenshot.
                Row("Remote", a.remote ?? "none", mono: true)
            }
            HStack(spacing: 8) {
                Button("Open folder") { controller.revealDataDirectory() }
                Button("Add desktop shortcuts") { controller.makeShortcuts() }
            }
            .font(.system(size: 12))
            .padding(.top, 2)
        }
    }

    /// The one section here that reports something no other surface does.
    ///
    /// Replacing the binaries does not restart the watcher, so this machine can
    /// be fully upgraded and still sync with the old version, publishing that
    /// version as its own to every other machine.
    private var backgroundSync: some View {
        Section("Background sync") {
            if let s = controller.about?.sync {
                Row("Schedule", s.scheduled ? "Installed" : "Not installed")
                if let secs = s.lastReportSecs {
                    Row("Last report", humanAge(secs))
                }
                if let running = s.runningVersion {
                    Row("Running", running)
                }
                if !s.scheduled {
                    Note("Nothing syncs in the background on this machine. Run jotbay schedule.",
                         tone: .warning)
                } else if s.restartNeeded {
                    Note("The background sync is still running \(s.runningVersion ?? "an older version"). "
                         + "Restart it to pick up \(controller.about?.version ?? "the new version").",
                         tone: .warning)
                }
            } else {
                Text("Loading").font(.system(size: 11)).foregroundStyle(.tertiary)
            }
        }
    }

    private var appearance: some View {
        Section("Appearance") {
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
            Note("Applies to this machine only. Settings never sync.", tone: .plain)
        }
    }

    private var advanced: some View {
        Section("Advanced") {
            Toggle("Show what each machine did", isOn: Binding(
                get: { controller.settings.rawActivity },
                set: { controller.updateSettings(rawActivity: $0) }
            ))
            .font(.system(size: 12))
            Note("The feed normally shows one line per change. Turn this on to see "
                 + "every commit, push and pull, per machine.", tone: .plain)

            Toggle("Verbose activity", isOn: Binding(
                get: { controller.settings.verbose },
                set: { controller.updateSettings(verbose: $0) }
            ))
            .font(.system(size: 12))
            .padding(.top, 4)
            Note("Show the raw underlying detail, including full git output on failures.",
                 tone: .plain)
            if let a = controller.about {
                Row("Updates from", a.toolRepo, mono: true)
                Button("Show the settings file") { controller.revealConfigFile() }
                    .font(.system(size: 12))
                    .padding(.top, 2)
            }
        }
    }

    private func humanAge(_ secs: Int) -> String {
        if secs < 60 { return "\(secs)s ago" }
        if secs < 3600 { return "\(secs / 60)m ago" }
        if secs < 86_400 { return "\(secs / 3600)h ago" }
        return "\(secs / 86_400)d ago"
    }
}

// MARK: - Settings panel building blocks

private struct Section<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            content
        }
    }
}

/// A label and a value. Values can be long paths, so they wrap and stay
/// selectable rather than being truncated into uselessness.
private struct Row: View {
    let label: String
    let value: String
    var mono = false

    init(_ label: String, _ value: String, mono: Bool = false) {
        self.label = label
        self.value = value
        self.mono = mono
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 84, alignment: .leading)
            Text(value)
                .font(.system(size: 11, design: mono ? .monospaced : .default))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct Note: View {
    enum Tone { case plain, warning }
    let text: String
    let tone: Tone

    init(_ text: String, tone: Tone) {
        self.text = text
        self.tone = tone
    }

    var body: some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(tone == .warning ? Color.orange : Color.secondary.opacity(0.8))
            .fixedSize(horizontal: false, vertical: true)
    }
}


/// Shown when the bundle on disk is no longer the one this process is running.
///
/// Without it the app keeps serving an old build indefinitely, and a bug that
/// was fixed and shipped looks like a bug that came back.
private struct RestartBanner: View {
    let version: String
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "arrow.clockwise.circle.fill")
                .foregroundStyle(Color.orange)
            Text("Version \(version) is installed. This window is still running the old one.")
                .font(.system(size: 12))
            Button("Restart Jotbay") { controller.restartIntoNewVersion() }
                .buttonStyle(.link)
                .font(.system(size: 12))
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color.orange.opacity(0.12))
    }
}

/// Offered rather than applied. The repository already carries the marker that
/// says a release exists, so noticing costs nothing; installing stays a choice.
///
/// One button, whatever this machine was installed with.
///
/// This used to hide the button for a cask or a `.deb` and print an
/// instruction instead, because `jotbay upgrade` refused to touch files it did
/// not own. That was honest but it left a person copying commands out of a
/// window. The engine now drives whichever installer owns those files, so the
/// button works everywhere and the instruction is gone.
private struct UpdateBanner: View {
    let version: String
    let about: About?
    @EnvironmentObject private var controller: JotbayController

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "arrow.down.circle.fill")
                .foregroundStyle(Color.accentColor)
            Text("Version \(version) is available.")
                .font(.system(size: 12))
            Button(controller.upgrading ? "Updating" : "Update now") {
                controller.upgrade()
            }
            .buttonStyle(.link)
            .font(.system(size: 12))
            .disabled(controller.upgrading)
            if controller.upgrading { ProgressView().controlSize(.small) }
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color.accentColor.opacity(0.10))
    }
}

/// Anything the app has to say, where the person can see it.
private struct MessageBar: View {
    let text: String
    let isError: Bool
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: isError ? "exclamationmark.circle.fill" : "info.circle.fill")
                .foregroundStyle(isError ? Color.red : Color.secondary)
            Text(text)
                .font(.system(size: 12))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background((isError ? Color.red : Color.secondary).opacity(0.10))
    }
}
