import AppKit
import RivalKit
import SwiftUI

/// The Settings window's state: the saved config from `rival config show`,
/// the unsaved edits, the proxy's model list and the last model check.
///
/// - Edits collect in `draft` (dotted keys) and go out in one
///   `rival config set --json` on Apply; the views show draft-over-saved.
/// - The key is not part of the draft: "Save key" stores it at once.
/// - The menu-bar "Check models" runs the same check as the Check tab and
///   posts `N of M ok` through `postNote`.
@MainActor @Observable
final class SettingsModel {
    enum CLIState {
        case locating
        case missing
        case ready(RivalCLI)
    }

    enum ProxyStatus {
        case unknown
        case loading
        case online(ProxyModels)
        case offline(String)
    }

    private(set) var cli: CLIState = .locating
    /// The saved config; nil until the first `show`.
    private(set) var saved: ConfigShow?
    /// Unsaved edits, keyed by dotted path.
    private(set) var draft: [String: JSONValue] = [:]
    /// The last failed read or write, shown under the form.
    var error: String?
    private(set) var busy = false
    private(set) var proxy: ProxyStatus = .unknown

    private(set) var checkRows: [CheckRow] = []
    private(set) var checkSummary: CheckSummary?
    private(set) var checkError: String?
    private(set) var checking = false
    private(set) var checkFinished: Date?

    /// Set by the app delegate: posts the menu-bar check notification.
    @ObservationIgnored var postNote: (@MainActor (FinishNote) -> Void)?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var checkTask: Task<Void, Never>?

    var hasDraft: Bool { !draft.isEmpty }

    var rival: RivalCLI? {
        if case .ready(let cli) = cli { return cli }
        return nil
    }

    // MARK: Loading

    /// Locates the CLI and reads the config, once per launch.
    func start() async {
        guard !started else { return }
        started = true
        await locate()
    }

    /// Locates again ("Try again" on the not-found screen).
    func locate() async {
        cli = .locating
        let found = await Task.detached { RivalCLI.locate() }.value
        cli = found.map { .ready($0) } ?? .missing
        if found != nil { await reload() }
    }

    func reload() async {
        guard let rival else { return }
        do {
            saved = try await rival.show()
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        await refreshProxy()
    }

    /// `config models` against the saved URL and key. Skipped while no
    /// route is on and no URL is set.
    func refreshProxy() async {
        guard let rival, let saved else { return }
        guard !saved.string("proxy.url").isEmpty else {
            proxy = .unknown
            return
        }
        proxy = .loading
        do {
            proxy = .online(try await rival.models())
        } catch {
            proxy = .offline(error.localizedDescription)
        }
    }

    // MARK: Values

    func value(_ key: String) -> JSONValue? { draft[key] ?? saved?.values[key]?.value }
    func string(_ key: String) -> String { value(key)?.stringValue ?? "" }
    func bool(_ key: String) -> Bool { value(key)?.boolValue ?? false }
    func list(_ key: String) -> [String] { value(key)?.stringList ?? [] }
    func source(_ key: String) -> String { draft[key] != nil ? "unsaved" : saved?.source(key) ?? "default" }

    /// Records an edit; an edit back to the saved value drops it.
    func set(_ key: String, _ value: JSONValue) {
        if saved?.values[key]?.value == value {
            draft[key] = nil
        } else {
            draft[key] = value
        }
    }

    func stringBinding(_ key: String) -> Binding<String> {
        Binding(get: { self.string(key) }, set: { self.set(key, .string($0)) })
    }

    func boolBinding(_ key: String) -> Binding<Bool> {
        Binding(get: { self.bool(key) }, set: { self.set(key, .bool($0)) })
    }

    /// `proxy` or `direct` for a model, with the draft applied.
    func route(_ entry: ModelEntry) -> String {
        guard let p = entry.provider, bool("proxy.\(p).enabled") else { return "direct" }
        return "proxy"
    }

    /// The id the model is called with, with the draft applied.
    func wire(_ entry: ModelEntry) -> String {
        guard let p = entry.provider, route(entry) == "proxy" else { return entry.model }
        return wireModel(prefix: string("proxy.\(p).model_prefix"), model: entry.model)
    }

    /// The prefix choices for a provider: the prefixes the proxy serves its
    /// models under, "" (none), and the current value.
    func prefixOptions(_ provider: String) -> [String] {
        var options: [String] = [""]
        if case .online(let models) = proxy {
            options += models.prefixes(serving: ModelCatalog.models(provider: provider))
        }
        options.append(string("proxy.\(provider).model_prefix"))
        var seen = Set<String>()
        return options.filter { seen.insert($0).inserted }
    }

    // MARK: Writes

    /// Saves the draft in one write. True when it saved (or had nothing to).
    @discardableResult
    func apply() async -> Bool {
        guard let rival, hasDraft else { return true }
        busy = true
        defer { busy = false }
        do {
            try await rival.set(patch: draft)
            draft = [:]
            error = nil
            await reload()
            return true
        } catch {
            self.error = error.localizedDescription
            return false
        }
    }

    func revert() {
        draft = [:]
        error = nil
    }

    func saveKey(_ key: String) async -> Bool {
        let key = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let rival, !key.isEmpty else { return false }
        busy = true
        defer { busy = false }
        do {
            try await rival.setKey(key)
            await reload()
            return true
        } catch {
            self.error = error.localizedDescription
            return false
        }
    }

    func clearKey() async {
        guard let rival else { return }
        busy = true
        defer { busy = false }
        do {
            try await rival.clearKey()
            await reload()
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: Check

    /// Runs `config check --json`; rows fill in as each model finishes.
    /// `applyFirst` saves the draft first and stops when that fails.
    /// `notify` posts the result (the menu-bar check).
    func runCheck(applyFirst: Bool = false, notify: Bool = false) {
        guard !checking else { return }
        checking = true
        checkTask = Task {
            await performCheck(applyFirst: applyFirst, notify: notify)
            checking = false
        }
    }

    func cancelCheck() {
        checkTask?.cancel()
    }

    private func performCheck(applyFirst: Bool, notify: Bool) async {
        if case .locating = cli { await start() }
        guard let rival else {
            if notify { postNote?(checkFailedNote("rival CLI not found — \(RivalLocator.installCommand)")) }
            return
        }
        if applyFirst, !(await apply()) { return }
        checkRows = []
        checkSummary = nil
        checkError = nil
        do {
            for try await event in rival.check() {
                switch event {
                case .row(let row): checkRows.append(row)
                case .summary(let summary): checkSummary = summary
                }
            }
            // A cancelled task ends the stream quietly; it does not throw.
            if Task.isCancelled {
                checkError = "cancelled"
                return
            }
            checkFinished = Date()
            if notify {
                let summary = checkSummary ?? CheckSummary(
                    proxy: CheckProxy(state: "off"), ok: checkRows.filter(\.ok).count, total: checkRows.count)
                postNote?(checkNote(rows: checkRows, summary: summary))
            }
        } catch {
            checkError = error.localizedDescription
            if notify { postNote?(checkFailedNote(error.localizedDescription)) }
        }
    }
}

/// The Settings scene (⌘,): Proxy, Models and Check tabs.
struct SettingsView: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        TabView {
            ProxyPane(settings: settings)
                .tabItem { Label("Proxy", systemImage: "network") }
            ModelsPane(settings: settings)
                .tabItem { Label("Models", systemImage: "cpu") }
            CheckPane(settings: settings)
                .tabItem { Label("Check", systemImage: "checkmark.seal") }
        }
        .frame(width: 680, height: 540)
        .task { await settings.start() }
    }
}

/// Shows `content` once the CLI is found; else a spinner or the
/// "rival CLI not found" screen with the install command.
struct CLIGate<Content: View>: View {
    @Bindable var settings: SettingsModel
    @ViewBuilder var content: () -> Content

    var body: some View {
        switch settings.cli {
        case .locating:
            ProgressView("Looking for rival…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .missing:
            CLIMissingView(settings: settings)
        case .ready:
            content()
        }
    }
}

struct CLIMissingView: View {
    let settings: SettingsModel

    var body: some View {
        ContentUnavailableView {
            Label("rival CLI not found", systemImage: "terminal")
        } description: {
            Text("Settings are read and saved through the rival command. Install it, then try again.")
        } actions: {
            VStack(spacing: 10) {
                HStack {
                    Text(RivalLocator.installCommand)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(RivalLocator.installCommand, forType: .string)
                    } label: {
                        Image(systemName: "doc.on.doc")
                    }
                    .help("Copy")
                }
                Button("Try Again") { Task { await settings.locate() } }
                Text("Searched RIVAL_BIN, /opt/homebrew/bin, /usr/local/bin, ~/.local/bin, ~/.cargo/bin and the login shell PATH.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
        }
    }
}

/// The bottom bar of the Proxy and Models tabs: the save state, the last
/// error, Revert and Apply.
struct ApplyBar: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        HStack(spacing: 8) {
            if let error = settings.error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .lineLimit(2)
                    .help(error)
            } else if settings.hasDraft {
                Text("Unsaved changes")
                    .foregroundStyle(.secondary)
            } else if let file = settings.saved?.configFile {
                Text(file)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            if settings.busy { ProgressView().controlSize(.small) }
            Button("Revert") { settings.revert() }
                .disabled(!settings.hasDraft || settings.busy)
            Button("Apply") { Task { await settings.apply() } }
                .keyboardShortcut(.defaultAction)
                .disabled(!settings.hasDraft || settings.busy)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
        .background(.bar)
    }
}
