import Foundation
import SwiftUI

/// Drives the `jotbay` CLI and publishes what it returns.
///
/// Everything the GUI knows comes from `jotbay --json`. Reimplementing git
/// handling in Swift would mean two conflict policies that could disagree.
@MainActor
final class JotbayController: ObservableObject {
    @Published var status: JotbayStatus = .empty
    @Published var activity: [ActivityEvent] = []
    @Published var isSyncing = false
    @Published var lastMessage: String = ""
    @Published var lastMessageIsError = false
    @Published var binaryMissing = false
    @Published var settings: AppSettings = .fallback
    @Published var needsSetup = false
    /// False until the first-run question has actually been answered. Without
    /// it the window paints the main UI for a frame and then swaps to setup,
    /// which reads as a glitch on exactly the launch that should feel most
    /// deliberate.
    @Published var setupChecked = false
    @Published var capabilities: SetupCapabilities?
    @Published var setupBusy = false
    @Published var setupError: String?
    /// True only between finishing setup and dismissing the confirmation, so
    /// the shortcut offer appears once rather than on every launch.
    @Published var justSetUp = false

    private var timer: Timer?

    /// Where the CLI might live, in preference order. The bundled copy wins so
    /// the app keeps working if the user's PATH does not include ~/.local/bin.
    private static var candidatePaths: [URL] {
        var paths: [URL] = []
        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("jotbay") {
            paths.append(bundled)
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        paths.append(home.appendingPathComponent(".local/bin/jotbay"))
        paths.append(URL(fileURLWithPath: "/usr/local/bin/jotbay"))
        paths.append(URL(fileURLWithPath: "/opt/homebrew/bin/jotbay"))
        return paths
    }

    private static func locateBinary() -> URL? {
        candidatePaths.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    var jotbayRoot: URL {
        status.root.isEmpty
            ? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("jotbay")
            : URL(fileURLWithPath: status.root)
    }

    var dataDirectory: URL { jotbayRoot.appendingPathComponent("data") }

    // MARK: - Lifecycle

    func start() {
        loadSettings()
        checkSetup()
        refresh(fetchRemote: true)
        // Fetches on every tick now that a watcher is doing the syncing: the
        // window's job is to show what the mesh is doing, and a local-only
        // repaint could sit twenty minutes behind the machine next to it.
        timer = Timer.scheduledTimer(withTimeInterval: 20, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh(fetchRemote: true) }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    // MARK: - Commands

    func refresh(fetchRemote: Bool) {
        Task {
            var args = ["status", "--json"]
            if !fetchRemote { args.append("--offline") }

            if let data = await run(args) {
                if let decoded = try? Self.decoder.decode(JotbayStatus.self, from: data) {
                    self.status = decoded
                }
            }
            var activityArgs = ["activity", "--json", "-n", "60"]
            if !fetchRemote { activityArgs.append("--offline") }
            if let data = await run(activityArgs),
               let decoded = try? Self.decoder.decode([ActivityEvent].self, from: data) {
                self.activity = decoded
            }
        }
    }

    func sync() {
        guard !isSyncing else { return }
        isSyncing = true
        lastMessage = "Syncing"
        lastMessageIsError = false

        Task {
            let data = await run(["sync", "--json"])
            if let data, let report = try? Self.decoder.decode(SyncReport.self, from: data) {
                self.lastMessage = report.summary
                self.lastMessageIsError = false
            } else if !self.binaryMissing {
                self.lastMessage = self.lastStderr.isEmpty ? "Sync failed" : self.lastStderr
                self.lastMessageIsError = true
            }
            self.isSyncing = false
            self.refresh(fetchRemote: false)
        }
    }

    /// An installed app opens onto first-run setup when there is no vault: a
    /// .dmg cannot clone a private repository, so somebody has to be asked.
    func checkSetup() {
        Task {
            if let data = await run(["init", "--json"]),
               let caps = try? JSONDecoder().decode(SetupCapabilities.self, from: data) {
                self.capabilities = caps
                // `init --json` reports what this machine can do, not whether a
                // vault exists; an offline status call is the cheapest test of
                // that, and it does not touch the network to answer it.
                self.needsSetup = await self.run(["status", "--offline", "--json"]) == nil
            }
            // Set on every path. A missing CLI is not a reason to sit on a
            // blank window forever. The main view says so plainly.
            self.setupChecked = true
        }
    }

    func runSetup(mode: String, value: String, location: String) {
        setupBusy = true
        setupError = nil
        Task {
            var args = ["init", "--at", location]
            switch mode {
            case "create": args += ["--create", value]
            case "clone":  args += ["--clone", value]
            default:       args += ["--adopt", location]
            }

            if await run(args) != nil {
                // Leave the machine syncing by itself. `jotbay init` does this
                // too, but setup here goes through run_setup rather than init,
                // so someone who arrived via the .dmg would otherwise finish
                // with a tool that syncs only when asked.
                _ = await run(["schedule"])
                // Not straight into the app: the shortcut offer only makes
                // sense now, when both targets finally exist.
                self.justSetUp = true
                self.needsSetup = false
                self.checkSetup()
                self.loadSettings()
                self.refresh(fetchRemote: true)
            } else {
                self.setupError = self.lastStderr.isEmpty
                    ? "Setup did not complete." : self.lastStderr
            }
            self.setupBusy = false
        }
    }

    /// Ends first run. Shortcuts are made through the CLI so there is one
    /// implementation of what a shortcut is on each platform.
    func finishSetup(app: Bool, notes: Bool) {
        Task {
            var failed: [String] = []
            if notes, await run(["shortcut", "notes"]) == nil { failed.append("your notes") }
            if app, await run(["shortcut", "app"]) == nil { failed.append("Jotbay") }
            if !failed.isEmpty {
                self.lastMessage =
                    "Could not make a shortcut to \(failed.joined(separator: " or "))."
                self.lastMessageIsError = true
            }
            // Dismiss either way: a missing icon is not worth trapping someone
            // on a screen they have already finished with.
            self.justSetUp = false
        }
    }

    func chooseFolder(startingAt: String) -> String? {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        panel.directoryURL = URL(fileURLWithPath: startingAt)
        return panel.runModal() == .OK ? panel.url?.path : nil
    }

    func loadSettings() {
        Task {
            if let data = await run(["settings", "--json"]),
               let decoded = try? JSONDecoder().decode(AppSettings.self, from: data) {
                self.settings = decoded
                self.applyAppearance(decoded.theme)
            }
        }
    }

    /// A SwiftUI app follows the system appearance unless told otherwise, so
    /// without this the theme picker would save a preference and change
    /// nothing on screen. nil hands control back to the system.
    private func applyAppearance(_ theme: String) {
        switch theme {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "dark":  NSApp.appearance = NSAppearance(named: .darkAqua)
        default:      NSApp.appearance = nil
        }
    }

    /// Written through the CLI rather than to the file directly, so there is
    /// one implementation of where settings live and what they mean.
    func updateSettings(theme: String? = nil, verbose: Bool? = nil) {
        Task {
            if let theme { _ = await run(["settings", "theme=\(theme)"]) }
            if let verbose { _ = await run(["settings", "verbose=\(verbose ? "on" : "off")"]) }
            self.loadSettings()
        }
    }

    func upgrade() {
        Task {
            self.lastMessage = "Downloading update"
            self.lastMessageIsError = false
            if await run(["upgrade"]) != nil {
                self.lastMessage = "Updated. Restart Jotbay to finish."
            } else {
                self.lastMessage = self.lastStderr.isEmpty ? "Update failed" : self.lastStderr
                self.lastMessageIsError = true
            }
            self.refresh(fetchRemote: false)
        }
    }

    func revealDataDirectory() {
        NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: dataDirectory.path)
    }

    func openTerminalDashboard() {
        guard let binary = Self.locateBinary() else { return }
        // `open -a Terminal` with a script keeps the TUI in a real terminal,
        // which is where a full-screen curses UI belongs.
        let script = "#!/bin/sh\nexec \(binary.path) dash\n"
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("jotbay-dash.command")
        try? script.write(to: tmp, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: tmp.path)
        NSWorkspace.shared.open(tmp)
    }

    // MARK: - Process plumbing

    private var lastStderr = ""

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            // The core emits RFC 3339 with fractional seconds sometimes and
            // without others, so try both rather than failing the whole decode.
            let withFraction = ISO8601DateFormatter()
            withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            if let date = withFraction.date(from: text) { return date }

            let plain = ISO8601DateFormatter()
            plain.formatOptions = [.withInternetDateTime]
            if let date = plain.date(from: text) { return date }

            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "unrecognised date: \(text)")
            )
        }
        return d
    }()

    private func run(_ arguments: [String]) async -> Data? {
        guard let binary = Self.locateBinary() else {
            binaryMissing = true
            lastMessage = "The jotbay command line tool is not installed"
            lastMessageIsError = true
            return nil
        }
        binaryMissing = false

        // No --jotbay when the vault is not yet known: the CLI's for_app-style
        // resolution reads the recorded setting, where this would hand it a
        // guess based on a default path.
        let root = status.root.isEmpty ? nil : status.root
        return await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                process.executableURL = binary
                process.arguments = root.map { arguments + ["--jotbay", $0] } ?? arguments

                let out = Pipe()
                let err = Pipe()
                process.standardOutput = out
                process.standardError = err

                do {
                    try process.run()
                } catch {
                    Task { @MainActor in self.lastStderr = error.localizedDescription }
                    continuation.resume(returning: nil)
                    return
                }

                // Read before waiting: a full pipe buffer would deadlock a
                // process that outputs more than 64KB.
                let data = out.fileHandleForReading.readDataToEndOfFile()
                let errData = err.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()

                let stderrText = String(data: errData, encoding: .utf8)?
                    .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                Task { @MainActor in self.lastStderr = stderrText }

                continuation.resume(returning: process.terminationStatus == 0 ? data : nil)
            }
        }
    }
}
