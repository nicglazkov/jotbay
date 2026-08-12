import SwiftUI

@main
struct JotbayApp: App {
    @StateObject private var controller = JotbayController()
    @Environment(\.openWindow) private var openWindow

    var body: some Scene {
        Window("Jotbay", id: "main") {
            RootView()
                .environmentObject(controller)
                .onAppear { controller.start() }
        }
        .defaultSize(width: 860, height: 560)
        .windowToolbarStyle(.unified(showsTitle: true))

        // A real window rather than a popover hanging off the toolbar.
        //
        // The panel had grown to five sections with selectable paths in it, and
        // a popover is the wrong container for that: it cannot be resized, it
        // dismisses when you click away to check something, and text you meant
        // to copy takes the panel with it. `Settings` is the macOS scene for
        // this, so it also arrives on Cmd+, and in the app menu for free.
        Settings {
            SettingsWindow()
                .environmentObject(controller)
        }

        // The menu bar item is the surface used ninety percent of the time;
        // .window style gives a real popover instead of a plain NSMenu.
        MenuBarExtra {
            MenuBarView()
                .environmentObject(controller)
        } label: {
            Image(nsImage: menuBarIcon)
        }
        .menuBarExtraStyle(.window)
    }

    /// The glyph reflects state at a glance: idle, mid-sync, or needs attention.
    /// Setup is its own case: before a vault exists the other three would all be
    /// lying, since there is nothing yet to be idle or busy about.
    private var menuBarIcon: NSImage {
        let name: String
        if controller.needsSetup {
            name = "attentionTemplate"
        } else if controller.isSyncing {
            name = "syncingTemplate"
        } else if controller.status.rebaseInProgress
            || controller.binaryMissing
            || controller.status.warnings.contains(where: { $0.severity == .blocked })
            || controller.status.nodes.contains(where: { $0.lastError != nil }) {
            name = "attentionTemplate"
        } else {
            name = "idleTemplate"
        }

        let image = NSImage(named: name) ?? NSImage(size: NSSize(width: 18, height: 18))
        image.isTemplate = true
        return image
    }
}
