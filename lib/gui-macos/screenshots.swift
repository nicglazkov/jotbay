// Renders the app's screens to PNG, offscreen, for the README.
//
//   lib/gui-macos/shots.sh
//
// Not part of the app. It compiles the real view files against a controller
// holding fixed demo data, so a screenshot cannot drift from the UI the way a
// hand-captured one does — regenerate it and any layout change shows up.
//
// The data is invented on purpose: hostnames from a real machine would put
// somebody's network in the README.
//
// Rendering goes through NSHostingView and CALayer.render rather than
// ImageRenderer, which returns a blank image for AppKit-backed content, and
// rather than a screen capture, which needs a recording permission.

import SwiftUI
import AppKit

@MainActor
func shot(_ name: String, _ view: some View, width: CGFloat, height: CGFloat, dark: Bool = false) {
    let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
    let host = NSHostingView(rootView: AnyView(view))
    host.appearance = appearance
    host.frame = NSRect(x: 0, y: 0, width: width, height: height)

    let window = NSWindow(contentRect: host.frame, styleMask: [.titled],
                          backing: .buffered, defer: false)
    window.appearance = appearance
    window.contentView = host
    window.setFrameOrigin(NSPoint(x: 20_000, y: 20_000))  // offscreen
    window.orderFrontRegardless()
    window.layoutIfNeeded()
    host.layoutSubtreeIfNeeded()
    host.wantsLayer = true
    RunLoop.main.run(until: Date().addingTimeInterval(1.0))

    let scale = 2
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: Int(width) * scale, pixelsHigh: Int(height) * scale,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
    ), let ctx = NSGraphicsContext(bitmapImageRep: rep) else {
        print("no bitmap: \(name)"); return
    }
    ctx.cgContext.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
    ctx.cgContext.translateBy(x: 0, y: height)   // CG origin is bottom-left
    ctx.cgContext.scaleBy(x: 1, y: -1)

    // The layer tree is transparent where the window background shows through,
    // so paint that first or light text lands on nothing.
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = ctx
    appearance.performAsCurrentDrawingAppearance {
        NSColor.windowBackgroundColor.setFill()
        NSRect(x: 0, y: 0, width: width, height: height).fill()
    }
    NSGraphicsContext.restoreGraphicsState()
    host.layer?.render(in: ctx.cgContext)

    guard let png = rep.representation(using: .png, properties: [:]) else {
        print("no png: \(name)"); return
    }
    let out = "docs/images/\(name).png"
    try? png.write(to: URL(fileURLWithPath: out))
    print("wrote \(out)")
}

// MARK: - Demo data

private func ago(_ seconds: TimeInterval) -> Date { Date().addingTimeInterval(-seconds) }

let demoHead = "7c1f9a4e2b83d05f16ae4c9821d7b3e6f0a5c284"

let demoNodes: [NodeStatus] = [
    NodeStatus(hostname: "studio-mac", os: "macos", arch: "aarch64",
               agentVersion: "1.4.0", lastSync: ago(120), head: demoHead,
               ahead: 0, behind: 0, dirty: 0, conflictsResolved: 0,
               lastError: nil, behindLocal: false),
    NodeStatus(hostname: "workstation", os: "windows", arch: "x86_64",
               agentVersion: "1.4.0", lastSync: ago(430), head: "3ab90f2",
               ahead: 0, behind: 1, dirty: 0, conflictsResolved: 1,
               lastError: nil, behindLocal: true),
    NodeStatus(hostname: "linux-desktop", os: "linux", arch: "x86_64",
               agentVersion: "1.4.0", lastSync: ago(900), head: demoHead,
               ahead: 0, behind: 0, dirty: 2, conflictsResolved: 0,
               lastError: nil, behindLocal: false),
    NodeStatus(hostname: "home-server", os: "linux", arch: "aarch64",
               agentVersion: "1.4.0", lastSync: ago(6 * 3600), head: "c4d1e88",
               ahead: 0, behind: 0, dirty: 0, conflictsResolved: 0,
               lastError: "could not reach the remote: name or service not known",
               behindLocal: false),
]

let demoStatus = JotbayStatus(
    root: "/Users/you/jotbay", branch: "main",
    head: demoHead, headShort: "7c1f9a4",
    ahead: 0, behind: 0, dirtyFiles: [], rebaseInProgress: false,
    conflicts: [], dataFiles: 218,
    warnings: [FileWarning(path: "data/media/walkthrough.mov",
                           bytes: 131_204_096, severity: .blocked,
                           advice: "Over GitHub's 100 MB limit. It stays where it is and everything else syncs.")],
    updateAvailable: nil, nodes: demoNodes)

let demoActivity: [ActivityEvent] = [
    ActivityEvent(at: ago(120), hostname: "studio-mac", kind: .changed,
                  summary: "Pushed 3 files", files: ["data/specs/api-notes.md",
                                                     "data/daily/2026-08-04.md",
                                                     "data/refs/postgres.md"],
                  detail: nil, head: demoHead),
    ActivityEvent(at: ago(430), hostname: "workstation", kind: .conflict,
                  summary: "1 conflict — both versions kept",
                  files: ["data/daily/2026-08-04.md",
                          "data/daily/2026-08-04.conflict-workstation-20260804T0212Z.md"],
                  detail: nil, head: "3ab90f2"),
    ActivityEvent(at: ago(900), hostname: "linux-desktop", kind: .changed,
                  summary: "Pulled 2 files", files: ["data/specs/api-notes.md",
                                                     "data/refs/postgres.md"],
                  detail: nil, head: demoHead),
    ActivityEvent(at: ago(6 * 3600), hostname: "home-server", kind: .error,
                  summary: "Sync failed: could not reach the remote",
                  files: nil,
                  detail: "fatal: unable to access 'https://github.com/you/notes.git/':\n"
                        + "Could not resolve host: github.com",
                  head: "c4d1e88"),
]

// MARK: - Entry point

@main struct Shots {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        app.finishLaunching()
        try? FileManager.default.createDirectory(atPath: "docs/images",
                                                 withIntermediateDirectories: true)

        let caps = SetupCapabilities(git: true, gh: true, ghAuthenticated: true,
                                     login: "you", defaultLocation: "/Users/you/jotbay",
                                     appInstalled: true, desktop: "/Users/you/Desktop")

        let c = JotbayController()
        c.setupChecked = true
        c.capabilities = caps

        // First run: no vault yet.
        c.needsSetup = true
        shot("macos-first-run", RootView().environmentObject(c), width: 760, height: 560)
        shot("macos-first-run-dark", RootView().environmentObject(c),
             width: 760, height: 560, dark: true)

        // The confirmation that follows setup.
        c.needsSetup = false
        c.justSetUp = true
        shot("macos-ready", RootView().environmentObject(c), width: 760, height: 560)

        // The main window, mid-life: four machines, one behind, one failing.
        c.justSetUp = false
        c.status = demoStatus
        c.activity = demoActivity
        shot("macos-main", RootView().environmentObject(c), width: 940, height: 620)
        shot("macos-main-dark", RootView().environmentObject(c),
             width: 940, height: 620, dark: true)

        shot("macos-menubar", MenuBarView().environmentObject(c), width: 300, height: 400)
    }
}
