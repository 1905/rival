import AppKit
import RivalKit
import SwiftUI

/// The right pane: breadcrumb header, Result / Raw / Prompt / Info, the member
/// picker for groups, and the Open-log / Stop toolbar.
struct RunDetail: View {
    let item: RunItem
    let store: SessionStore
    @Binding var toast: String?

    @State private var detail = RunDetailModel()
    @State private var stopRequest: StopRequest?
    /// The member's log tail, loaded here so the toolbar knows whether the
    /// file exists without a stat per body pass.
    @State private var log = LogSnapshot.loading

    private let inspector = SystemProcessInspector()

    var body: some View {
        let member = detail.member(in: item)
        // A snapshot of another member's log is never shown.
        let snapshot = member.map { log.path == $0.logFile ? log : .loading } ?? .loading
        let polling = detail.tab == .raw && (member.map { isLive($0.status) } ?? false)
        VStack(alignment: .leading, spacing: 0) {
            DetailHeader(item: item, follow: detail.follow) { detail.setFollow(!detail.follow) }
                .padding(.horizontal, 14)
                .padding(.top, 10)
                .padding(.bottom, 8)

            HStack(spacing: 12) {
                Segmented(selection: $detail.tab, options: DetailTab.allCases) { $0.rawValue }
                Spacer(minLength: 8)
                if item.isGroup {
                    Segmented(
                        selection: Binding(
                            get: { member?.id ?? "" },
                            set: { detail.selectMember(id: $0, in: item) }
                        ),
                        options: item.sessions.map(\.id)
                    ) { id in
                        item.sessions.first { $0.id == id }.map(memberLabel) ?? id
                    }
                }
            }
            .font(Mono.body)
            .padding(.horizontal, 14)
            .padding(.bottom, 8)

            Divider().overlay(Theme.dim)

            if let member {
                switch detail.tab {
                case .result:
                    ResultPane(
                        session: member,
                        snapshot: snapshot,
                        onOpenRaw: { detail.tab = .raw }
                    )
                    .id(member.id)
                case .raw:
                    OutputPane(
                        session: member,
                        snapshot: snapshot,
                        follow: detail.follow,
                        onUserScroll: { detail.userScrolled(atBottom: $0) }
                    )
                    .id(member.id)
                case .prompt:
                    PromptPane(session: member, store: store)
                        .id(member.id)
                case .info:
                    InfoPane(session: member)
                }
            } else {
                EmptyState(title: "Empty run", detail: "This run has no sessions.")
            }
        }
        .background(Theme.background)
        .onChange(of: item, initial: true) { _, new in detail.sync(new) }
        // Re-read when the member's record changes, and every second while it
        // is live and the Raw tab shows it. Otherwise it loads once, for the
        // toolbar's file check, so tab switches on a finished member do not
        // re-read. A finished member's tail is also parsed for Result; an
        // unchanged file reuses the last parse.
        .task(id: LogKey(session: member, polling: polling)) {
            guard let member else { return }
            let live = isLive(member.status)
            // The read runs detached and ignores cancellation, so a stale read
            // must not overwrite a newer snapshot: check after every await.
            let first = await LogSnapshot.load(member.logFile, parse: !live, previous: log)
            if Task.isCancelled { return }
            log = first
            while polling && !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                if Task.isCancelled { break }
                let next = await LogSnapshot.load(member.logFile)
                if Task.isCancelled { break }
                log = next
            }
        }
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    if let path = member?.logFile, !path.isEmpty {
                        NSWorkspace.shared.open(URL(fileURLWithPath: path))
                    }
                } label: {
                    Label("Open full log", systemImage: "doc.text.magnifyingglass")
                }
                .labelStyle(.titleAndIcon)
                .disabled(!snapshot.exists)
                .help("Open the member's whole log file")

                Button(role: .destructive) {
                    requestStop()
                } label: {
                    Label("Stop…", systemImage: "stop.circle")
                }
                .labelStyle(.titleAndIcon)
                .disabled(stopCandidates(item).isEmpty)
                .help("SIGTERM the run's live processes")
            }
        }
        .sheet(item: $stopRequest) { req in
            StopConfirm(request: req) {
                stopRequest = nil
                toast = performStop(
                    runID: req.runID, confirmedIDs: Set(req.targets.map(\.session.id)), runs: store.runs,
                    root: store.root, inspector: inspector, signaller: SystemSignaller()
                )
            } cancel: {
                stopRequest = nil
            }
        }
    }

    private func requestStop() {
        let candidates = stopCandidates(item)
        guard !candidates.isEmpty else {
            toast = "nothing running"
            return
        }
        stopRequest = StopRequest(
            runID: item.id,
            title: "\(runKind(item)) \(runShortID(item)) · \(projectName(item.primary?.workDir ?? ""))",
            targets: candidates.map { .init(session: $0, state: stopTargetState($0, inspector: inspector)) }
        )
    }
}

private struct LogKey: Hashable {
    let session: Session?
    let polling: Bool
}

// MARK: - Header

/// " rival › project › kind id8 … glyph status elapsed   follow ●"
struct DetailHeader: View {
    let item: RunItem
    let follow: Bool
    let toggleFollow: () -> Void

    var body: some View {
        let status = runStatus(item)
        let live = status.isLive
        HStack(spacing: 0) {
            RivalWord()
            Text(" › ").foregroundStyle(Theme.dim)
            Text(projectName(item.primary?.workDir ?? "")).foregroundStyle(Theme.fg)
            Text(" › ").foregroundStyle(Theme.dim)
            Text("\(runKind(item)) \(runShortID(item))").foregroundStyle(Theme.fg)
                .textSelection(.enabled)
            Spacer(minLength: 16)
            HStack(spacing: 6) {
                StatusGlyph(status: status)
                Text(status.rawValue)
                if live {
                    TimelineView(.periodic(from: .now, by: 1)) { ctx in Text(runElapsed(item, now: ctx.date)) }
                } else {
                    Text(runElapsed(item, now: Date()))
                }
            }
            .foregroundStyle(Theme.color(for: status))
            Button(action: toggleFollow) {
                HStack(spacing: 4) {
                    Text("follow").foregroundStyle(Theme.dim)
                    Text(follow ? "●" : "○").foregroundStyle(follow ? Theme.accent : Theme.fg)
                }
            }
            .buttonStyle(.plain)
            .padding(.leading, 18)
            .help(follow ? "Following the log tail. Scroll up to pause." : "Resume following the log tail")
        }
        .font(Mono.bold)
        .lineLimit(1)
    }
}

// MARK: - Raw

/// The member's log tail, as `RunDetail` loaded it.
struct OutputPane: View {
    let session: Session
    let snapshot: LogSnapshot
    let follow: Bool
    let onUserScroll: (Bool) -> Void

    var body: some View {
        VStack(spacing: 0) {
            if snapshot.truncated {
                HStack(spacing: 8) {
                    Text("earlier output omitted —").foregroundStyle(Theme.dim)
                    Button("Open full log") {
                        NSWorkspace.shared.open(URL(fileURLWithPath: session.logFile))
                    }
                    .buttonStyle(.link)
                    Spacer()
                }
                .font(Mono.small)
                .padding(.horizontal, 14)
                .padding(.vertical, 5)
                Divider().overlay(Theme.dim)
            }
            LogView(
                text: snapshot.text,
                placeholder: snapshot.placeholder,
                trailer: session.status == "failed" ? session.error.map { "\n\nerror:\n" + sanitizeLog($0) } : nil,
                follow: follow,
                onUserScroll: onUserScroll
            )
        }
    }
}

struct LogSnapshot: Equatable, Sendable {
    /// The log file this was read from; nil for `loading`.
    var path: String?
    /// The file existed at the last read.
    var exists = false
    var text = ""
    var truncated = false
    /// A dim note shown instead of an empty log.
    var placeholder: String?
    /// The parsed answer, when the load asked for it and the file was read.
    var result: RunResult?
    /// The file's size and modification time at the read.
    var stamp: FileStamp?

    struct FileStamp: Equatable, Sendable {
        let size: UInt64
        let modified: Date
    }

    static let loading = LogSnapshot()

    /// Reads and sanitizes the tail off the main thread. With `parse`, also
    /// parses the raw (unsanitized) tail for the Result tab, reusing
    /// `previous.result` when the file's path, size and mtime are unchanged.
    static func load(_ path: String, parse: Bool = false, previous: LogSnapshot? = nil) async -> LogSnapshot {
        await Task.detached(priority: .userInitiated) {
            guard !path.isEmpty else {
                return LogSnapshot(path: path, placeholder: "(no log file recorded)")
            }
            guard FileManager.default.fileExists(atPath: path) else {
                return LogSnapshot(path: path, placeholder: "(log unavailable: no such file)")
            }
            do {
                let stamp = fileStamp(path)
                let tail = try readTail(path: path, maxBytes: maxTailBytes)
                let text = sanitizeLog(tail.text)
                var result: RunResult?
                if parse {
                    if let previous, let stamp, previous.path == path, previous.stamp == stamp,
                       let cached = previous.result {
                        result = cached
                    } else {
                        result = parseRunResult(raw: tail.text)
                    }
                }
                return LogSnapshot(path: path, exists: true, text: text, truncated: tail.truncated,
                                   placeholder: text.isEmpty ? "(empty log)" : nil, result: result, stamp: stamp)
            } catch {
                return LogSnapshot(path: path, exists: true,
                                   placeholder: "(log unavailable: \(error.localizedDescription))")
            }
        }.value
    }
}

private func fileStamp(_ path: String) -> LogSnapshot.FileStamp? {
    guard let attrs = try? FileManager.default.attributesOfItem(atPath: path),
          let size = (attrs[.size] as? NSNumber)?.uint64Value,
          let modified = attrs[.modificationDate] as? Date else { return nil }
    return .init(size: size, modified: modified)
}

// MARK: - Prompt

struct PromptPane: View {
    let session: Session
    let store: SessionStore
    @State private var loaded: (id: String, text: String)?

    var body: some View {
        let text = loaded?.id == session.id ? loaded?.text : nil
        LogView(text: text ?? "", placeholder: text == nil ? "" : nil, trailer: nil, follow: false, onUserScroll: nil)
            .task(id: session.id) {
                let full = await store.fullSession(id: session.id)?.prompt
                if let full, !full.isEmpty {
                    loaded = (session.id, sanitizeLog(full))
                } else {
                    let preview = sanitizeLog(session.promptPreview ?? "")
                    loaded = (session.id, (preview.isEmpty ? "" : preview + "\n") + "(full prompt unavailable)")
                }
            }
    }
}

// MARK: - Info

struct InfoPane: View {
    let session: Session

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 14, verticalSpacing: 4) {
                    ForEach(infoRows(session, now: Date()), id: \.label) { row in
                        GridRow {
                            Text(row.label).foregroundStyle(Theme.dim)
                            Text(row.value)
                                .foregroundStyle(valueColor(row))
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                if let err = session.error {
                    SessionErrorView(error: err)
                }
            }
            .font(Mono.body)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.background)
    }

    private func valueColor(_ row: InfoRow) -> Color {
        switch row.label {
        case "status": return Theme.color(for: RunStatus(rawValue: session.status) ?? .unknown)
        case "id", "model": return Theme.accent
        default: return Theme.fg
        }
    }
}

/// "error:" and the session's sanitized error in the failure colour, for the
/// Info tab and the Result tab's parse-failure note.
struct SessionErrorView: View {
    let error: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("error:")
            Text(sanitizeLog(error)).fixedSize(horizontal: false, vertical: true)
        }
        .foregroundStyle(Theme.fail)
        .textSelection(.enabled)
        .padding(.top, 4)
    }
}

// MARK: - Stop

struct StopRequest: Identifiable {
    struct Target: Identifiable {
        let session: Session
        let state: StopTargetState
        var id: String { session.id }
    }

    let id = UUID()
    let runID: String
    let title: String
    let targets: [Target]
}

private func stopTargetLabel(_ state: StopTargetState) -> String {
    switch state {
    case .signal: return "SIGTERM"
    case .gone: return "process gone"
    case .unverified: return "cannot verify process identity"
    }
}

struct StopConfirm: View {
    let request: StopRequest
    let confirm: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Stop \(request.title)?").font(Mono.bold)
            VStack(alignment: .leading, spacing: 4) {
                ForEach(request.targets) { t in
                    HStack(spacing: 8) {
                        StatusGlyph(status: RunStatus(rawValue: t.session.status) ?? .unknown)
                        Text(memberLabel(t.session))
                        Text("pid \(t.session.pid)").foregroundStyle(Theme.dim)
                        Text(stopTargetLabel(t.state))
                            .foregroundStyle(t.state == .signal ? Theme.running : Theme.dim)
                    }
                }
            }
            .font(Mono.body)
            Text("A live process gets SIGTERM only while its PID start time still matches the run. A run without a recorded start time is not signalled.")
                .font(Mono.small)
                .foregroundStyle(Theme.dim)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel, action: cancel)
                    .keyboardShortcut(.cancelAction)
                Button("Stop", role: .destructive, action: confirm)
            }
        }
        .padding(20)
        .frame(minWidth: 460)
        .background(Theme.background)
        .foregroundStyle(Theme.fg)
        .preferredColorScheme(.dark)
    }
}
