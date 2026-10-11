import AppKit
import Foundation
import Observation
import RivalKit
import UserNotifications

/// `userInfo` key carrying the finished run's `RunItem.id`.
private let runIDKey = "runID"

/// Posts "✓ review orbit-web · gpt-6-astra · 6m24s" when a run finishes.
///
/// - It watches every store snapshot through Observation, so it works with
///   the main window closed.
/// - `FinishDetector` decides what finished; the first snapshot is only a
///   baseline.
/// - Authorization is requested on the first finish, not at launch.
/// - Clicking a notification selects the run and opens the main window.
/// - `UNUserNotificationCenter` needs an .app bundle; a bare `swift run`
///   binary gets no notifications and a hint in the menu bar instead.
@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    private let app: AppModel
    private let center: UNUserNotificationCenter?
    private var detector = FinishDetector()

    init(app: AppModel) {
        self.app = app
        // `.current()` throws an Objective-C exception outside a bundle.
        let bundled = Bundle.main.bundleURL.pathExtension == "app" && Bundle.main.bundleIdentifier != nil
        center = bundled ? UNUserNotificationCenter.current() : nil
        super.init()
        center?.delegate = self
    }

    func start() {
        guard let center else {
            app.notificationHint = "notifications need Rival.app (not a bare binary)"
            observe()
            return
        }
        // Reading the settings never prompts; it only shows a hint for an
        // earlier denial.
        center.getNotificationSettings { settings in
            let status = settings.authorizationStatus
            Task { @MainActor [weak self] in self?.apply(status) }
        }
        observe()
    }

    /// Re-arms after each change: Observation fires once per registration.
    private func observe() {
        withObservationTracking {
            _ = app.store.revision
        } onChange: { [weak self] in
            // onChange runs before the new value is stored; hop to read it.
            Task { @MainActor [weak self] in
                self?.snapshotChanged()
                self?.observe()
            }
        }
    }

    private func snapshotChanged() {
        // Revision 0 is the empty pre-scan state, not a snapshot.
        guard app.store.revision > 0 else { return }
        let finished = detector.update(app.store.runs)
        guard !finished.isEmpty, UserDefaults.standard.bool(forKey: notifyOnFinishKey) else { return }
        let notes = finished.map { finishNote($0) }
        Task { await post(notes) }
    }

    /// Posts the menu-bar model check result ("✓ model check · 5 of 5 ok").
    /// Not gated by "Notify on finish": the user asked for this one. The
    /// menu-bar popover shows the same result inline.
    func postCheck(_ note: FinishNote) {
        Task { await post([note]) }
    }

    private func post(_ notes: [FinishNote]) async {
        guard let center else { return }
        var status = await authorizationStatus(center)
        if status == .notDetermined {
            let granted = await withCheckedContinuation { cont in
                center.requestAuthorization(options: [.alert, .sound]) { ok, _ in cont.resume(returning: ok) }
            }
            status = granted ? .authorized : .denied
        }
        apply(status)
        guard status == .authorized || status == .provisional else { return }
        notes.forEach { enqueue($0, center) }
    }

    private func enqueue(_ note: FinishNote, _ center: UNUserNotificationCenter) {
        let content = UNMutableNotificationContent()
        content.title = note.title
        content.body = note.body
        content.sound = .default
        content.userInfo = [runIDKey: note.runID]
        // One notification per run: a re-finish replaces the old banner.
        center.add(UNNotificationRequest(identifier: note.runID, content: content, trigger: nil))
    }

    private func authorizationStatus(_ center: UNUserNotificationCenter) async -> UNAuthorizationStatus {
        await withCheckedContinuation { cont in
            center.getNotificationSettings { cont.resume(returning: $0.authorizationStatus) }
        }
    }

    private func apply(_ status: UNAuthorizationStatus) {
        app.notificationHint = status == .denied
            ? "notifications blocked: allow Rival in System Settings › Notifications"
            : nil
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let runID = response.notification.request.content.userInfo[runIDKey] as? String
        Task { @MainActor in
            if runID == checkNoteID {
                self.app.showSettings()
            } else {
                self.app.show(runID: runID)
            }
        }
        completionHandler()
    }

    /// Show the banner even while Rival is the active app.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }
}
