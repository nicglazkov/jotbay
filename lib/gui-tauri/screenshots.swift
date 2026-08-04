// Renders the Windows/Linux front end to PNG, offscreen, for the README.
//
//   lib/gui-tauri/shots.sh
//
// The front end is plain HTML/CSS/JS that gets all its state from Tauri's
// `invoke`. Rather than hand-editing the DOM — which would screenshot a page
// the app never actually produces — this injects a stub `window.__TAURI__`
// before app.js runs and answers each command with demo data. The real render
// path then runs untouched.
//
// Without the stub, app.js throws on its very first line (`window.__TAURI__.core`
// is undefined), every later declaration is skipped, and the page stays blank.

import AppKit
import WebKit

let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    .appendingPathComponent("lib/gui-tauri/src")
let page = root.appendingPathComponent("index.html")
let outDir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    .appendingPathComponent("docs/images")

// Demo data. Invented hostnames on purpose: a real machine's name in the README
// puts somebody's network in it.
let demoJSON = """
window.__DEMO__ = {
  status: {
    root: "/home/you/jotbay", branch: "main",
    head: "7c1f9a4e2b83d05f16ae4c9821d7b3e6f0a5c284", head_short: "7c1f9a4",
    ahead: 0, behind: 0, dirty_files: [], rebase_in_progress: false,
    conflicts: [], data_files: 218,
    warnings: [{ path: "data/media/walkthrough.mov", bytes: 131204096,
                 severity: "blocked",
                 advice: "Over GitHub's 100 MB limit. It stays where it is and everything else syncs." }],
    update_available: null,
    nodes: [
      { hostname: "studio-mac", os: "macos", arch: "aarch64", agent_version: "1.4.0",
        last_sync: ISO(120), head: "7c1f9a4e2b83d05f16ae4c9821d7b3e6f0a5c284",
        ahead: 0, behind: 0, dirty: 0, conflicts_resolved: 0, last_error: null, behind_local: false },
      { hostname: "workstation", os: "windows", arch: "x86_64", agent_version: "1.4.0",
        last_sync: ISO(430), head: "3ab90f2aa0",
        ahead: 0, behind: 1, dirty: 0, conflicts_resolved: 1, last_error: null, behind_local: true },
      { hostname: "linux-desktop", os: "linux", arch: "x86_64", agent_version: "1.4.0",
        last_sync: ISO(900), head: "7c1f9a4e2b83d05f16ae4c9821d7b3e6f0a5c284",
        ahead: 0, behind: 0, dirty: 2, conflicts_resolved: 0, last_error: null, behind_local: false },
      { hostname: "home-server", os: "linux", arch: "aarch64", agent_version: "1.4.0",
        last_sync: ISO(21600), head: "c4d1e8800a",
        ahead: 0, behind: 0, dirty: 0, conflicts_resolved: 0,
        last_error: "could not reach the remote: name or service not known", behind_local: false }
    ]
  },
  activity: [
    { at: ISO(120), hostname: "studio-mac", kind: "changed", summary: "Pushed 3 files",
      files: ["data/specs/api-notes.md", "data/daily/2026-08-04.md", "data/refs/postgres.md"],
      detail: null, head: "7c1f9a4" },
    { at: ISO(430), hostname: "workstation", kind: "conflict",
      summary: "1 conflict - both versions kept",
      files: ["data/daily/2026-08-04.md",
              "data/daily/2026-08-04.conflict-workstation-20260804T0212Z.md"],
      detail: null, head: "3ab90f2" },
    { at: ISO(900), hostname: "linux-desktop", kind: "changed", summary: "Pulled 2 files",
      files: ["data/specs/api-notes.md", "data/refs/postgres.md"], detail: null, head: "7c1f9a4" },
    { at: ISO(21600), hostname: "home-server", kind: "error",
      summary: "Sync failed: could not reach the remote", files: null,
      detail: "fatal: unable to access 'https://github.com/you/notes.git/'", head: "c4d1e88" }
  ],
  capabilities: { git: true, gh: true, gh_authenticated: true, login: "you",
                  default_location: "/home/you/jotbay", app_installed: true,
                  desktop: "/home/you/Desktop" }
};
"""

/// Injected before app.js so its first line finds what it expects.
func bridge(hasVault: Bool) -> String {
    """
    function ISO(secondsAgo) { return new Date(Date.now() - secondsAgo * 1000).toISOString(); }
    \(demoJSON)
    window.__TAURI__ = {
      core: {
        invoke: async (cmd, args) => {
          const d = window.__DEMO__;
          switch (cmd) {
            case "get_setup_state": return { has_vault: \(hasVault), capabilities: d.capabilities };
            case "get_status":      return d.status;
            case "get_activity":    return d.activity;
            case "get_settings":    return { theme: "system", verbose: false };
            case "data_dir":        return "/home/you/jotbay/data";
            case "get_nodes":       return d.status.nodes;
            default:                return null;
          }
        }
      },
      dialog: { open: async () => null }
    };
    """
}

final class Shooter: NSObject, WKNavigationDelegate {
    let web: WKWebView
    let window: NSWindow
    let after: String
    let out: String
    var done = false

    init(width: CGFloat, height: CGFloat, dark: Bool, hasVault: Bool, after: String, out: String) {
        self.after = after
        self.out = out
        let cfg = WKWebViewConfiguration()
        cfg.userContentController.addUserScript(
            WKUserScript(source: bridge(hasVault: hasVault),
                         injectionTime: .atDocumentStart, forMainFrameOnly: true))
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: width, height: height), configuration: cfg)
        window = NSWindow(contentRect: web.frame, styleMask: [.titled],
                          backing: .buffered, defer: false)
        super.init()
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = web
        window.setFrameOrigin(NSPoint(x: 20_000, y: 20_000))
        window.orderFrontRegardless()
        web.navigationDelegate = self
        web.loadFileURL(page, allowingReadAccessTo: root)
    }

    func webView(_ w: WKWebView, didFinish nav: WKNavigation!) {
        // The boot sequence is async; let it settle before nudging and shooting.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
            w.evaluateJavaScript(self.after) { _, err in
                if let err { print("js: \(err)") }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
                    let cfg = WKSnapshotConfiguration()
                    cfg.rect = w.bounds
                    w.takeSnapshot(with: cfg) { img, err in
                        defer { self.done = true }
                        guard let img, let tiff = img.tiffRepresentation,
                              let rep = NSBitmapImageRep(data: tiff),
                              let png = rep.representation(using: .png, properties: [:]) else {
                            print("snapshot failed: \(err?.localizedDescription ?? "?")"); return
                        }
                        try? png.write(to: URL(fileURLWithPath: self.out))
                        print("wrote \(self.out)")
                    }
                }
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.finishLaunching()
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

let jobs: [(String, Bool, Bool, String, CGFloat)] = [
    // name, dark, hasVault, post-load script, height
    ("tauri-main", false, true, "", 620),
    ("tauri-main-dark", true, true, "", 620),
    ("tauri-first-run", false, false, "", 560),
]

for (name, dark, hasVault, after, height) in jobs {
    let s = Shooter(width: 940, height: height, dark: dark, hasVault: hasVault,
                    after: after, out: outDir.appendingPathComponent("\(name).png").path)
    let deadline = Date().addingTimeInterval(20)
    while !s.done && Date() < deadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }
}
