import SwiftUI
import AppKit

/// The app's own icon, at whatever size the caller asks for.
///
/// Resolved from the bundled `jotbay.icns` first and only then from
/// `NSApp.applicationIconImage`. Both give the same artwork in a real bundle,
/// but the application icon is whatever AppKit decides it is, outside a bundle
/// that is the generic document icon, which is exactly the wrong thing to show
/// on the first screen a new user ever sees.
struct AppIcon: View {
    var size: CGFloat = 64

    var body: some View {
        Image(nsImage: Self.image)
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }

    static let image: NSImage = {
        if let named = Bundle.main.image(forResource: NSImage.Name("jotbay")) {
            return named
        }
        if let url = Bundle.main.url(forResource: "jotbay", withExtension: "icns"),
           let fromFile = NSImage(contentsOf: url) {
            return fromFile
        }
        return NSApp.applicationIconImage
    }()
}
