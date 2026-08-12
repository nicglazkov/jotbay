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
    /// The folded feed: one entry per change rather than per machine. Used
    /// unless the raw setting is on.
    @Published var changes: [Change] = []
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
    /// The version now sitting in /Applications, set once this process is no
    /// longer running it. Nil while the running app is the installed one.
    @Published var replacedOnDisk: String?
    /// Everything the settings panel shows. Loaded when the panel opens rather
    /// than on every tick, because none of it changes minute to minute.
    @Published var about: About?
    @Published var checkingUpdates = false
    @Published var upgrading = false
    @Published var updateCheckResult: String?

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

    /// Kept in step with `jotbay_core::git::looks_offline`.
    ///
    /// Duplicated deliberately and kept short: this reads the stderr of a CLI
    /// call that already failed, before any JSON exists to carry a flag. The
    /// engine remains the authority for what every machine *publishes*; this
    /// only decides the wording of one line in this window.
    static func looksOffline(_ text: String) -> Bool {
        let t = text.lowercased()
        let signs = [
            "could not resolve host", "could not resolve proxy",
            "temporary failure in name resolution", "name or service not known",
            "nodename nor servname", "network is unreachable", "network is down",
            "no route to host", "connection timed out", "operation timed out",
            "timed out after", "connection refused", "connection reset by peer",
            "failed to connect to", "unable to access", "ssl connect error",
            "the remote end hung up unexpectedly",
        ]
        return signs.contains { t.contains($0) }
    }

    // MARK: - Detecting that the bundle was replaced underneath us

    /// Which file this process is actually running, recorded at launch.
    ///
    /// macOS lets an app bundle be replaced while the app runs: the process
    /// keeps the image it started with, and the new one sits on disk unused
    /// until a relaunch. Homebrew, a DMG, and `jotbay upgrade` all do this. A
    /// person then keeps clicking a button whose fix shipped a day ago.
    ///
    /// Comparing version strings is not enough, because a reinstall of the
    /// same version swaps the file too and would compare equal. The file
    /// identity is the only signal that catches every case, so that is what
    /// gets recorded.
    private static let launchedFile: FileID? = fileID(of: Bundle.main.executableURL)

    private struct FileID: Equatable {
        let device: dev_t
        let inode: ino_t
    }

    private static func fileID(of url: URL?) -> FileID? {
        guard let path = url?.path else { return nil }
        var info = stat()
        guard stat(path, &info) == 0 else { return nil }
        return FileID(device: info.st_dev, inode: info.st_ino)
    }

    /// The version of the bundle on disk right now, read from the file rather
    /// than from `Bundle.main`, whose info dictionary is cached from launch.
    private static func versionOnDisk() -> String? {
        let plist = Bundle.main.bundleURL.appendingPathComponent("Contents/Info.plist")
        guard let data = try? Data(contentsOf: plist),
              let parsed = try? PropertyListSerialization.propertyList(from: data, format: nil),
              let dict = parsed as? [String: Any]
        else { return nil }
        return dict["CFBundleShortVersionString"] as? String
    }

    /// Nil out rather than latch: an install that is still in progress leaves
    /// the executable missing for a moment, and reporting that as a stale
    /// process would be a banner that never goes away.
    func checkForReplacedBundle() {
        guard let launched = Self.launchedFile,
              let current = Self.fileID(of: Bundle.main.executableURL)
        else { return }
        replacedOnDisk = launched == current ? nil : (Self.versionOnDisk() ?? "A newer version")
    }

    /// Start the installed copy, then stand down. The new instance has to be
    /// asked for explicitly, because macOS activates the running one instead
    /// of launching the replacement otherwise.
    func restartIntoNewVersion() {
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: config) { _, _ in
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }

    var jotbayRoot: URL {
        status.root.isEmpty
            ? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("jotbay")
            : URL(fileURLWithPath: status.root)
    }

    /// Where the notes are, as the engine reports them.
    ///
    /// This used to append "data" to the root. That was a second copy of a rule
    /// that lives in the engine, and when vaults became flat it pointed at a
    /// directory which no longer existed. NSWorkspace declines to open a missing
    /// path silently, so the folder button stopped doing anything at all.
    ///
    /// Falls back to the root rather than to "data": a vault whose status has
    /// not loaded yet is better served by opening something real.
    var dataDirectory: URL {
        if let notes = status.notes, !notes.isEmpty {
            return URL(fileURLWithPath: notes)
        }
        return jotbayRoot
    }

    // MARK: - Lifecycle

    func start() {
        // Touch this now, while the bundle is certainly still the one we
        // launched from. A lazy static read after a replacement would record
        // the new file as the original and never report anything.
        _ = Self.launchedFile
        loadSettings()
        loadAbout()
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
        checkForReplacedBundle()
        Task {
            var args = ["status", "--json"]
            if !fetchRemote { args.append("--offline") }

            if let data = await run(args) {
                if let decoded = try? Self.decoder.decode(JotbayStatus.self, from: data) {
                    self.status = decoded
                }
            }
            // Both shapes come from `activity --json`; --raw decides which.
            // Fetched according to the setting so the window never holds a
            // feed the user is not looking at.
            var activityArgs = ["activity", "--json", "-n", "60"]
            if !fetchRemote { activityArgs.append("--offline") }
            if self.settings.rawActivity {
                activityArgs.append("--raw")
                if let data = await run(activityArgs),
                   let decoded = try? Self.decoder.decode([ActivityEvent].self, from: data) {
                    self.activity = decoded
                }
            } else if let data = await run(activityArgs),
                      let decoded = try? Self.decoder.decode([Change].self, from: data) {
                self.changes = decoded
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
                // Being off the network is not a failure to report in red. The
                // work is already committed locally, because sync commits
                // before it touches the network, so there is nothing at risk
                // and nothing for the reader to do.
                if Self.looksOffline(self.lastStderr) {
                    self.lastMessage = "Offline. Your work is saved here and will sync when the network is back."
                    self.lastMessageIsError = false
                } else {
                    self.lastMessage = self.lastStderr.isEmpty ? "Sync failed" : self.lastStderr
                    self.lastMessageIsError = true
                }
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
    func updateSettings(theme: String? = nil, verbose: Bool? = nil, rawActivity: Bool? = nil) {
        Task {
            if let theme { _ = await run(["settings", "theme=\(theme)"]) }
            if let verbose { _ = await run(["settings", "verbose=\(verbose ? "on" : "off")"]) }
            if let rawActivity {
                _ = await run(["settings", "raw_activity=\(rawActivity ? "on" : "off")"])
            }
            self.loadSettings()
            // The feed is a different shape in each mode, so it has to be
            // fetched again rather than re-rendered.
            self.refresh(fetchRemote: false)
        }
    }

    func upgrade() {
        guard !upgrading else { return }
        upgrading = true
        Task {
            self.lastMessage = "Updating"
            self.lastMessageIsError = false
            if let data = await run(["upgrade", "--json"]),
               let outcome = try? Self.decoder.decode(UpgradeOutcome.self, from: data) {
                var message = outcome.alreadyCurrent
                    ? "Already on \(outcome.version)."
                    : "Updated to \(outcome.version)."
                if !outcome.alreadyCurrent && !outcome.syncRestarted {
                    message += " No background sync is set up on this machine."
                }
                if outcome.restartApp {
                    message += " Restart Jotbay to use it."
                }
                self.lastMessage = message
                self.lastMessageIsError = false
                self.loadAbout()
                self.refresh(fetchRemote: false)
            } else if await run(["upgrade"]) != nil {
                // An older engine that does not know --json still upgrades.
                self.lastMessage = "Updated. Restart Jotbay to finish."
                self.lastMessageIsError = false
                self.loadAbout()
            } else {
                // The engine's refusal for a managed install is an instruction,
                // not a fault, so it is worth showing plainly rather than in red.
                let managed = self.about?.upgradeInPlace == false
                self.lastMessage = self.lastStderr.isEmpty
                    ? (self.about?.upgradeInstructions ?? "Update failed")
                    : self.lastStderr
                self.lastMessageIsError = !managed
            }
            self.upgrading = false
            self.refresh(fetchRemote: false)
        }
    }

    func loadAbout() {
        Task {
            if let data = await run(["about", "--json"]),
               let decoded = try? Self.decoder.decode(About.self, from: data) {
                self.about = decoded
            }
        }
    }

    /// Refresh the release marker, then re-read. `status` is what fetches the
    /// marker, so this is a real check rather than a re-read of what was
    /// already known.
    func checkForUpdates() {
        guard !checkingUpdates else { return }
        checkingUpdates = true
        updateCheckResult = nil
        Task {
            _ = await run(["status", "--json"])
            if let data = await run(["about", "--json"]),
               let decoded = try? Self.decoder.decode(About.self, from: data) {
                self.about = decoded
                self.updateCheckResult = decoded.updateAvailable
                    .map { "Version \($0) is available" } ?? "This is the newest version"
            } else {
                self.updateCheckResult = "Could not check"
            }
            self.checkingUpdates = false
        }
    }

    /// Open the file that holds the preferences, for the settings this window
    /// deliberately does not offer a control for.
    func revealConfigFile() {
        guard let path = about?.configPath else { return }
        NSWorkspace.shared.selectFile(path, inFileViewerRootedAtPath:
            (path as NSString).deletingLastPathComponent)
    }

    func makeShortcuts() {
        Task {
            if await run(["shortcut"]) != nil {
                self.lastMessage = "Shortcuts added to the desktop"
            } else {
                self.lastMessage = self.lastStderr.isEmpty ? "Could not make shortcuts" : self.lastStderr
                self.lastMessageIsError = true
            }
        }
    }

    func openReleasesPage() {
        let repo = about?.toolRepo ?? "nicglazkov/jotbay"
        if let url = URL(string: "https://github.com/\(repo)/releases/latest") {
            NSWorkspace.shared.open(url)
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
