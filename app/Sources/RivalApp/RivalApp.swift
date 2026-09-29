import AppKit
import RivalKit
import SwiftUI

/// `UserDefaults` key of the menu bar's "Notify on finish" toggle.
let notifyOnFinishKey = "notifyOnFinish"

/// State shared by every scene: the one session store and the selected run.
///
/// The main window, the menu bar extra and the finish notifier all hold this
/// same object. A notification or menu bar click sets `selectedRunID` and
/// brings the window forward; the list and the detail follow it.
@MainActor @Observable
final class AppModel {
    static let mainWindowID = "main"

    let store: SessionStore
    /// `RunItem.id` of the selected run. Survives refreshes and re-sorts.
    var selectedRunID: String?
    /// A dim line for the menu bar popover when finish notifications cannot be
    /// shown (permission denied, or the binary runs outside an .app bundle).
    var notificationHint: String?
    /// SwiftUI's `openWindow`, captured from the first view that appears.
    /// Needed to reopen the main window after the user closed it.
    @ObservationIgnored var openWindowAction: OpenWindowAction?

    init(store: SessionStore) {
        self.store = store
    }

    /// The selected run in the current snapshot, even when the filter hides it.
    var selectedRun: RunItem? {
        guard let id = selectedRunID else { return nil }
        return store.runs.first { $0.id == id }
    }

    /// Selects `runID` (when given) and opens or fronts the main window.
    func show(runID: String?) {
        if let runID { selectedRunID = runID }
        showMainWindow()
    }

    func showMainWindow() {
        NSApp.activate()
        if let open = openWindowAction {
            // A `Window` scene: fronts the window, or reopens it when closed.
            open(id: Self.mainWindowID)
        } else if let window = NSApp.windows.first(where: { $0.identifier?.rawValue.hasPrefix(Self.mainWindowID) == true }) {
            window.makeKeyAndOrderFront(nil)
        }
    }
}

/// Owns the model and the notifier, so both exist before the first scene is
/// built and survive every window closing. Also sets the notification
/// delegate before launch finishes, so a click that launched the app is seen.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let watchdog: MemoryWatchdog
    let model: AppModel
    let notifier: Notifier

    override init() {
        // First, before anything that could run away: quits the app above
        // 1 GB footprint (see MemoryWatchdog).
        let env = ProcessInfo.processInfo.environment
        var limit = MemoryWatchdog.defaultLimit
        var firstScanDelay = Duration.zero
        #if DEBUG
        // Soak test hooks (app/scripts/soak_test.py). Not compiled into
        // release builds. RIVAL_WATCHDOG_LIMIT_MB lowers the limit to prove
        // the watchdog fires; RIVAL_DEBUG_SCAN_DELAY holds back the store's
        // start (first scan, watcher and poll) so the loading state stays on
        // screen.
        if let mb = env["RIVAL_WATCHDOG_LIMIT_MB"].flatMap(UInt64.init), mb > 0 {
            limit = mb << 20
        }
        if let secs = env["RIVAL_DEBUG_SCAN_DELAY"].flatMap(Int.init), secs > 0 {
            firstScanDelay = .seconds(secs)
        }
        #endif
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
        let watchdog = MemoryWatchdog(limit: limit, version: version)
        watchdog.start()
        self.watchdog = watchdog

        UserDefaults.standard.register(defaults: [notifyOnFinishKey: true])
        let store = SessionStore(root: RivalPaths.root(environment: env))
        let model = AppModel(store: store)
        #if DEBUG
        // Dev aid for fixture screenshots of the dev bundle: preselect a run
        // by `RunItem.id` ("solo:<session id>" or "group:<group id>"). Not
        // compiled into release builds. `dev_bundle.py --select` prints the
        // launch line.
        model.selectedRunID = env["RIVAL_SELECT"].flatMap { $0.isEmpty ? nil : $0 }
        #endif
        self.model = model
        self.notifier = Notifier(app: model)
        super.init()
        if firstScanDelay > .zero {
            Task { try? await Task.sleep(for: firstScanDelay); store.start() }
        } else {
            store.start()
        }
        notifier.start()
    }

    /// Closing the window leaves the app running in the menu bar.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// A Dock click with no window open brings the main window back.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { model.showMainWindow() }
        return true
    }
}

@main
struct RivalMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        // A single `Window`, not a `WindowGroup`: "Open Rival" and a
        // notification click must front the one window, never add a second.
        Window("Rival", id: AppModel.mainWindowID) {
            MainWindow(app: delegate.model)
        }
        .defaultSize(width: 1180, height: 760)
        .commands {
            // ⌘F / ⌘G / ⇧⌘G drive the Output log's native find bar.
            CommandGroup(replacing: .textEditing) {
                Button("Find in Log…") { LogFind.shared.perform(.showFindInterface) }
                    .keyboardShortcut("f")
                Button("Find Next") { LogFind.shared.perform(.nextMatch) }
                    .keyboardShortcut("g")
                Button("Find Previous") { LogFind.shared.perform(.previousMatch) }
                    .keyboardShortcut("g", modifiers: [.command, .shift])
            }
        }

        MenuBarExtra {
            MenuBarView(app: delegate.model)
        } label: {
            MenuBarLabel(store: delegate.model.store)
        }
        .menuBarExtraStyle(.window)
    }
}
