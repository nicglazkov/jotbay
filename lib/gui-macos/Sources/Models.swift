import Foundation

// Mirrors of the JSON emitted by `jotbay --json`. The CLI is the single source
// of truth for sync behaviour; these types exist only to read its output.

struct NodeStatus: Codable, Identifiable, Hashable {
    var id: String { hostname }

    let hostname: String
    let os: String
    let arch: String
    let agentVersion: String
    let lastSync: Date
    let head: String
    let ahead: Int
    let behind: Int
    let dirty: Int
    let conflictsResolved: Int
    let lastError: String?
    /// The last failure was this machine being unable to reach the remote.
    /// Optional so a node on an older agent, which never publishes it, decodes.
    let offline: Bool?
    /// Strictly behind the local head. Optional so a node still running an
    /// older agent, whose published status lacks the field, still decodes.
    let behindLocal: Bool?

    enum CodingKeys: String, CodingKey {
        case hostname, os, arch, head, ahead, behind, dirty
        case agentVersion = "agent_version"
        case lastSync = "last_sync"
        case conflictsResolved = "conflicts_resolved"
        case behindLocal = "behind_local"
        case lastError = "last_error"
        case offline
    }

    /// Kept in step with `NodeHealth::health` in the Rust core.
    ///
    /// "Diverged" used to cover every head mismatch, which overstated the
    /// common case: right after this machine pushes, every peer shows a
    /// different head purely because it has not pulled yet. `behindLocal` is
    /// computed remotely by `git merge-base --is-ancestor`.
    func health(localHead: String, interval: TimeInterval = 300) -> NodeHealth {
        // Before the error check, which is also set. A machine with no route to
        // the remote is not a machine with a problem, and painting an ordinary
        // commute red is how red stops meaning anything.
        if offline == true { return .offline }
        if lastError != nil { return .error }
        // Before the ordinary staleness check. A machine that cannot publish
        // its own failure, because publishing needs the credentials that
        // failed, leaves silence as the only signal there is.
        if Date().timeIntervalSince(lastSync) > 24 * 60 * 60 { return .missing }
        if Date().timeIntervalSince(lastSync) > interval * 3 { return .stale }
        if head != localHead { return (behindLocal ?? false) ? .behind : .diverged }
        return .healthy
    }

    var shortHead: String { String(head.prefix(7)) }
}

enum NodeHealth {
    case healthy, behind, diverged, stale, missing, offline, error

    var label: String {
        switch self {
        case .healthy: return "in sync"
        case .behind: return "behind"
        case .diverged: return "diverged"
        case .stale: return "not answering"
        case .missing: return "silent for over a day"
        case .offline: return "offline"
        case .error: return "error"
        }
    }
}

/// A file that will not sync, or that will cost more than the user expects.
struct FileWarning: Codable, Identifiable, Hashable {
    var id: String { path }

    let path: String
    let bytes: Int64
    let severity: Severity

    /// Sent by `jotbay_core::limits`, so this window and the CLI always give
    /// the same advice about the same file. Optional only so an older `jotbay`
    /// binary that predates the field still decodes.
    let advice: String?

    var humanSize: String {
        let mb = Double(bytes) / (1024 * 1024)
        if mb >= 1024 { return String(format: "%.1f GB", mb / 1024) }
        if mb >= 1 { return String(format: "%.0f MB", mb) }
        return String(format: "%.0f KB", Double(bytes) / 1024)
    }

    var filename: String { (path as NSString).lastPathComponent }
}

enum Severity: String, Codable {
    case blocked, warning, advisory
}

struct JotbayStatus: Codable {
    let root: String
    /// Where the notes are. Not always `root`, and never worth guessing: this
    /// view used to append "data" itself, which broke the folder button the day
    /// vaults became flat.
    let notes: String?
    let branch: String
    let head: String
    let headShort: String
    let ahead: Int
    let behind: Int
    let dirtyFiles: [String]
    let rebaseInProgress: Bool
    let conflicts: [String]
    let dataFiles: Int
    let warnings: [FileWarning]
    let updateAvailable: String?
    let nodes: [NodeStatus]

    enum CodingKeys: String, CodingKey {
        case root, notes, branch, head, ahead, behind, conflicts, nodes
        case headShort = "head_short"
        case dirtyFiles = "dirty_files"
        case rebaseInProgress = "rebase_in_progress"
        case dataFiles = "data_files"
        case warnings
        case updateAvailable = "update_available"
    }

    var isClean: Bool {
        dirtyFiles.isEmpty && ahead == 0 && behind == 0 && !rebaseInProgress
    }

    static let empty = JotbayStatus(
        root: "", notes: nil, branch: "", head: "", headShort: "",
        ahead: 0, behind: 0, dirtyFiles: [], rebaseInProgress: false,
        conflicts: [], dataFiles: 0, warnings: [], updateAvailable: nil, nodes: []
    )
}

struct ConflictResolution: Codable, Hashable {
    let path: String
    let keptCopy: String?
    let kind: String

    enum CodingKeys: String, CodingKey {
        case path, kind
        case keptCopy = "kept_copy"
    }
}

struct SyncReport: Codable {
    let committed: Bool
    let commitMessage: String?
    let pulled: Int
    let pushed: Bool
    let conflicts: [ConflictResolution]
    let headShort: String
    let skippedLocked: Bool

    enum CodingKeys: String, CodingKey {
        case committed, pulled, pushed, conflicts
        case commitMessage = "commit_message"
        case headShort = "head_short"
        case skippedLocked = "skipped_locked"
    }

    var didNothing: Bool { !committed && pulled == 0 && !pushed && conflicts.isEmpty }

    /// A one-line description for the menu bar and toasts.
    var summary: String {
        if skippedLocked { return "Another sync is already running" }
        if didNothing { return "Already in sync" }

        var parts: [String] = []
        if committed { parts.append("committed") }
        if pulled > 0 { parts.append("pulled \(pulled)") }
        if !conflicts.isEmpty {
            parts.append("\(conflicts.count) conflict\(conflicts.count == 1 ? "" : "s"), both versions kept")
        }
        if pushed { parts.append("pushed") }
        return parts.joined(separator: " · ").capitalizedFirst
    }
}

/// One thing that happened on one machine. Distinct from `CommitInfo`: a
/// commit says what changed, an event says what a machine *did*, including
/// failing, which leaves no commit behind.
struct ActivityEvent: Codable, Identifiable, Hashable {
    var id: String { "\(hostname)-\(at.timeIntervalSince1970)-\(summary)" }

    let at: Date
    let hostname: String
    let kind: EventKind
    let summary: String
    /// Paths this event touched, so a row can answer "pushed 2 files, which?"
    let files: [String]?
    /// Raw underlying text. Shown only in verbose mode; a push rejected for a
    /// private email is five lines of git stderr that helps almost nobody.
    let detail: String?
    let head: String

    var id2: String { "\(hostname)-\(at.timeIntervalSince1970)" }
}

/// One thing that happened to the notes, folded across every machine that
/// reported it. What `jotbay activity --json` returns unless raw mode is on.
struct Change: Codable, Identifiable, Hashable {
    var id: String { "\(at.timeIntervalSince1970)-\(summary)" }

    let at: Date
    let kind: ChangeKind
    let summary: String
    let files: [String]
    /// The machine that made it, when the commit report is still in the buffer.
    let origin: String?
    /// Every machine that has reported this change.
    let machines: [String]
    /// Above one only for a condition that kept repeating.
    let repeats: Int
    let detail: String?
}

enum ChangeKind: String, Codable {
    case updated, conflict, offline, problem

    /// Distinct per kind. One pair of arrows for everything meant the feed
    /// looked identical whether a note had synced or a push had been rejected.
    var symbol: String {
        switch self {
        case .updated: return "doc.text.fill"
        case .conflict: return "exclamationmark.triangle.fill"
        case .offline: return "wifi.slash"
        case .problem: return "xmark.octagon.fill"
        }
    }
}

/// What `jotbay init --json` reports a machine can offer, so the first-run
/// screen can disable a route rather than let someone pick one that fails.
struct SetupCapabilities: Codable {
    let git: Bool
    let gh: Bool
    let ghAuthenticated: Bool
    let login: String?
    let defaultLocation: String
    let appInstalled: Bool
    let desktop: String

    enum CodingKeys: String, CodingKey {
        case git, gh, login, desktop
        case ghAuthenticated = "gh_authenticated"
        case defaultLocation = "default_location"
        case appInstalled = "app_installed"
    }
}

/// What `jotbay about --json` reports. The settings panel renders this rather
/// than assembling the same facts from four places.
struct About: Codable {
    let version: String
    let hostname: String
    let os: String
    let arch: String
    let root: String
    let notes: String
    let branch: String
    let remote: String?
    let files: Int
    let configPath: String
    let toolRepo: String
    let updateAvailable: String?
    /// Whether `jotbay upgrade` can replace this copy at all. False for a
    /// cask, a .deb, or anything inside Jotbay.app.
    let upgradeInPlace: Bool
    /// What to do instead, written by the engine.
    let upgradeInstructions: String?
    let sync: SyncHealth

    enum CodingKeys: String, CodingKey {
        case version, hostname, os, arch, root, notes, branch, remote, files, sync
        case configPath = "config_path"
        case toolRepo = "tool_repo"
        case updateAvailable = "update_available"
        case upgradeInPlace = "upgrade_in_place"
        case upgradeInstructions = "upgrade_instructions"
    }
}

/// Whether the background sync is installed, and whether it is the version
/// this machine has installed. The two can differ: replacing the binaries does
/// not restart the watcher, and the watcher is what publishes this machine's
/// version to everyone else.
struct SyncHealth: Codable {
    let scheduled: Bool
    let runningVersion: String?
    let lastReportSecs: Int?
    let restartNeeded: Bool

    enum CodingKeys: String, CodingKey {
        case scheduled
        case runningVersion = "running_version"
        case lastReportSecs = "last_report_secs"
        case restartNeeded = "restart_needed"
    }
}

/// What `jotbay upgrade --json` reports it did.
struct UpgradeOutcome: Codable {
    let version: String
    let replaced: [String]
    /// The background sync now runs the new version. False when this machine
    /// has none registered.
    let syncRestarted: Bool
    /// This window is still the old build, because a running process keeps
    /// the image it started with.
    let restartApp: Bool
    /// Nothing was done because nothing needed doing.
    let alreadyCurrent: Bool

    enum CodingKeys: String, CodingKey {
        case version, replaced
        case syncRestarted = "sync_restarted"
        case restartApp = "restart_app"
        case alreadyCurrent = "already_current"
    }
}

/// A search result. Name matches come first and carry no line.
struct Hit: Codable, Identifiable, Hashable {
    var id: String { path + (excerpt ?? "") }
    let path: String
    let line: Int?
    let excerpt: String?
    let nameMatch: Bool

    enum CodingKeys: String, CodingKey {
        case path, line, excerpt
        case nameMatch = "name_match"
    }
}

/// One version of one note.
struct Version: Codable, Identifiable, Hashable {
    var id: String { sha }
    let sha: String
    let short: String
    let at: Date
    let machine: String?
    let subject: String
    let deleted: Bool
}

/// A note that was removed and can be brought back.
struct DeletedNote: Codable, Identifiable, Hashable {
    var id: String { path + sha }
    let path: String
    let sha: String
    let short: String
    let at: Date
    let machine: String?
}

/// A conflict copy still sitting in the vault, paired with its note.
struct ConflictPair: Codable, Identifiable, Hashable {
    var id: String { copy }
    let original: String
    let copy: String
    let machine: String?
    let at: String?
    let identical: Bool
}

/// Per-machine preferences, read from the same file the CLI writes.
struct AppSettings: Codable {
    var theme: String
    var verbose: Bool
    /// Show what each machine did instead of what changed.
    var rawActivity: Bool

    enum CodingKeys: String, CodingKey {
        case theme, verbose
        case rawActivity = "raw_activity"
    }

    static let fallback = AppSettings(theme: "system", verbose: false, rawActivity: false)
}

enum EventKind: String, Codable {
    case changed
    case conflict
    case error
    case offline

    var symbol: String {
        switch self {
        case .changed: return "arrow.up.arrow.down"
        case .conflict: return "exclamationmark.triangle.fill"
        case .error: return "xmark.octagon.fill"
        case .offline: return "wifi.slash"
        }
    }
}

struct CommitInfo: Codable, Identifiable, Hashable {
    var id: String { sha }

    let sha: String
    let short: String
    let subject: String
    let author: String
    let timestamp: String
    let node: String?
}

extension String {
    var capitalizedFirst: String {
        guard let first else { return self }
        return first.uppercased() + dropFirst()
    }
}
