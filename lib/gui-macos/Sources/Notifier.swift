import Foundation
import UserNotifications

/// System notifications for the two things worth interrupting someone over.
///
/// The app's premise is telling you when something is wrong, but once the
/// window is closed the only signal was a menu bar glyph you had to happen to
/// look at. An iMac on this fleet was broken for four days behind that glyph.
///
/// Two rules keep this from becoming noise. Only conditions a person has to
/// act on notify: a sync failure, a conflict, a machine gone silent. And each
/// condition notifies once when it starts, not once per poll; the watcher
/// retries every 20 seconds, and a notification per retry is the activity-feed
/// spam bug all over again, this time in Notification Center.
@MainActor
final class Notifier {
    static let shared = Notifier()

    /// Conditions currently notified about, so each fires once per onset.
    /// Keyed by a stable description of the condition, not the message: a
    /// retry produces the same key, a new problem produces a new one.
    private var active: Set<String> = []
    private var authorized = false

    private init() {}

    /// Ask once, quietly. `.provisional` delivers to Notification Center
    /// without a permission dialog, so the first run is not interrupted; the
    /// person promotes them to alerts if they prove worth it.
    func prepare() {
        UNUserNotificationCenter.current().requestAuthorization(
            options: [.alert, .provisional]
        ) { granted, _ in
            Task { @MainActor in self.authorized = granted }
        }
    }

    /// Reconcile what is wrong now against what was wrong last time, notifying
    /// on the new ones and clearing the healed ones.
    func reconcile(problems: [(key: String, title: String, body: String)]) {
        guard authorized else { return }
        let current = Set(problems.map(\.key))

        for problem in problems where !active.contains(problem.key) {
            let content = UNMutableNotificationContent()
            content.title = problem.title
            content.body = problem.body
            let request = UNNotificationRequest(
                identifier: problem.key,
                content: content,
                trigger: nil
            )
            UNUserNotificationCenter.current().add(request)
        }

        // A healed condition leaves the set, so its return later is news again.
        // Its notification is also withdrawn: a banner about a problem that no
        // longer exists is a small lie on screen.
        let healed = active.subtracting(current)
        if !healed.isEmpty {
            UNUserNotificationCenter.current()
                .removeDeliveredNotifications(withIdentifiers: Array(healed))
        }
        active = current
    }
}
