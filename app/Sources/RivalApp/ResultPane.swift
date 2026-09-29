import RivalKit
import SwiftUI

/// The Result tab: the member's final answer as a header card plus finding
/// cards grouped by severity, or as rendered markdown. A live member, a
/// missing log and a parse failure each get a short note and "Open Raw".
struct ResultPane: View {
    let session: Session
    let snapshot: LogSnapshot
    let onOpenRaw: () -> Void

    var body: some View {
        Group {
            if isLive(session.status) {
                ResultNote(text: "Run is still going — the result appears when it finishes.", onOpenRaw: onOpenRaw)
            } else if let result = snapshot.result {
                switch result {
                case let .findings(summary, rating, findings):
                    FindingsView(session: session, summary: summary, rating: rating, findings: findings)
                case let .markdown(text):
                    MarkdownAnswer(session: session, text: text)
                case let .failed(reason):
                    ResultNote(title: "Couldn't parse this run's output.", text: reason, error: session.error,
                               onOpenRaw: onOpenRaw)
                }
            } else if let placeholder = snapshot.placeholder {
                ResultNote(text: placeholder, onOpenRaw: onOpenRaw)
            } else {
                // Still reading; the load is quick, so no spinner.
                Color.clear
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.background)
    }
}

// MARK: - Header

/// "model · effort · mode · duration", the rating on the right, the severity
/// chips, and the summary, in one framed card. `groups` nil leaves out the
/// counts row.
private struct HeaderCard: View {
    let session: Session
    var rating: Int?
    var groups: [SeverityGroup]?
    var summary = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(meta).foregroundStyle(Theme.accent)
                Spacer(minLength: 12)
                if let rating {
                    Text("rating \(rating)/10").font(Mono.bold).foregroundStyle(ratingColor(rating))
                }
            }
            if let groups {
                if groups.isEmpty {
                    Text("No findings.").foregroundStyle(Theme.dim)
                } else {
                    HStack(spacing: 14) {
                        ForEach(groups, id: \.severity) { g in
                            HStack(spacing: 4) {
                                Text("●").foregroundStyle(severityColor(g.severity))
                                Text("\(g.findings.count) \(g.severity)")
                            }
                        }
                    }
                    .font(Mono.small)
                }
            }
            if !summary.isEmpty {
                Text(summary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(Theme.dim, lineWidth: 1))
    }

    private var meta: String {
        [modelName(session), session.effort, session.mode, sessionElapsed(session, now: Date())]
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
    }

    private func ratingColor(_ r: Int) -> Color {
        switch r {
        case ...4: return Theme.fail
        case 5...7: return Theme.running
        default: return Theme.accent
        }
    }
}

/// Critical and high in the failure colour, medium amber, the rest dim.
private func severityColor(_ severity: String) -> Color {
    switch severityRank(severity) {
    case 0, 1: return Theme.fail
    case 2: return Theme.running
    default: return Theme.dim
    }
}

// MARK: - Findings

private struct FindingsView: View {
    let session: Session
    let summary: String
    let rating: Int?
    let groups: [SeverityGroup]
    let rows: [FindingRow]

    init(session: Session, summary: String, rating: Int?, findings: [Finding]) {
        self.session = session
        self.summary = summary
        self.rating = rating
        groups = severityGroups(findings)
        rows = findingRows(groups)
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                HeaderCard(session: session, rating: rating, groups: groups, summary: summary)
                // One flat list with unique ids. Nested ForEach blocks inside a
                // LazyVStack mis-measured and left a screen-high gap after the
                // first section.
                ForEach(rows, id: \.id) { row in
                    switch row.content {
                    case .rule(let severity):
                        SectionRule(title: severity.uppercased(), color: severityColor(severity))
                            .padding(.top, 6)
                    case .card(let f):
                        FindingCard(finding: f)
                    }
                }
            }
            .font(Mono.body)
            .foregroundStyle(Theme.fg)
            .textSelection(.enabled)
            .padding(14)
        }
    }
}

/// One row of the findings list: a severity rule or a finding card.
private struct FindingRow {
    enum Content { case rule(String), card(Finding) }
    let id: String
    let content: Content
}

/// `groups` flattened to rows, ids unique across the whole list.
private func findingRows(_ groups: [SeverityGroup]) -> [FindingRow] {
    groups.flatMap { g in
        [FindingRow(id: "rule:\(g.severity)", content: .rule(g.severity))]
            + g.findings.enumerated().map { i, f in FindingRow(id: "card:\(g.severity):\(i)", content: .card(f)) }
    }
}

/// " CRITICAL ────────"
private struct SectionRule: View {
    let title: String
    let color: Color

    var body: some View {
        HStack(spacing: 8) {
            Text(title).font(Mono.bold).foregroundStyle(color)
            Rectangle().fill(Theme.dim).frame(height: 1)
        }
    }
}

private struct FindingCard: View {
    let finding: Finding

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 0) {
                Text(location).foregroundStyle(Theme.accent)
                Text(meta).foregroundStyle(Theme.dim)
            }
            .font(Mono.small)
            if !finding.title.isEmpty {
                Text(finding.title).font(Mono.bold).fixedSize(horizontal: false, vertical: true)
            }
            if !finding.body.isEmpty {
                Text(finding.body).fixedSize(horizontal: false, vertical: true)
            }
            if let s = finding.failureScenario { Detail(title: "Failure scenario", text: s) }
            if let s = finding.suggestion { Detail(title: "Suggestion", text: s) }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .leading) {
            Rectangle().fill(severityColor(finding.severity)).frame(width: 2)
        }
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(Theme.dim, lineWidth: 1))
    }

    private var location: String {
        if finding.file.isEmpty { return finding.line > 0 ? "line \(finding.line)" : "—" }
        return finding.line > 0 ? "\(finding.file):\(finding.line)" : finding.file
    }

    private var meta: String {
        var parts: [String] = []
        if !finding.category.isEmpty { parts.append(finding.category) }
        if finding.confidence > 0 { parts.append("conf \(finding.confidence)") }
        if severityRank(finding.severity) == severityNames.count, !finding.severity.isEmpty {
            parts.append(finding.severity)
        }
        return parts.map { " · " + $0 }.joined()
    }
}

/// A collapsed section of a finding card.
private struct Detail: View {
    let title: String
    let text: String
    @State private var open = false

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            Text(text)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 2)
        } label: {
            Text(title).font(Mono.small).foregroundStyle(Theme.dim)
                .contentShape(Rectangle())
                .onTapGesture { open.toggle() }
        }
    }
}

// MARK: - Markdown

private struct MarkdownAnswer: View {
    let session: Session
    let blocks: [MarkdownBlock]

    init(session: Session, text: String) {
        self.session = session
        blocks = markdownBlocks(text)
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 8) {
                HeaderCard(session: session)
                    .padding(.bottom, 4)
                ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                    MarkdownBlockView(block: block)
                }
            }
            .font(Mono.body)
            .foregroundStyle(Theme.fg)
            .textSelection(.enabled)
            .padding(14)
        }
    }
}

private struct MarkdownBlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case let .heading(level, text):
            Text(inlineMarkdown(text))
                .font(Mono.bold)
                .foregroundStyle(level <= 2 ? Theme.accent : Theme.fg)
                .padding(.top, level <= 2 ? 6 : 2)
                .fixedSize(horizontal: false, vertical: true)
        case let .item(marker, text, indent):
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker).foregroundStyle(Theme.dim)
                Text(inlineMarkdown(text)).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, CGFloat(indent) * 16)
        case let .code(text):
            Text(text)
                .font(Mono.small)
                .fixedSize(horizontal: false, vertical: true)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.dim.opacity(0.18))
                .clipShape(RoundedRectangle(cornerRadius: 4))
        case let .paragraph(text):
            Text(inlineMarkdown(text)).fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Inline markdown (bold, italic, `code`, links) with code spans in the accent
/// colour. Text the parser rejects shows as it is.
private func inlineMarkdown(_ s: String) -> AttributedString {
    guard var a = try? AttributedString(
        markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    ) else { return AttributedString(s) }
    let code = a.runs.filter { $0.inlinePresentationIntent?.contains(.code) == true }.map(\.range)
    for r in code { a[r].foregroundColor = Theme.accent }
    return a
}

// MARK: - Notes

/// A dim note with an "Open Raw" link, for states without a result. With a
/// `title` (a parse failure) the title leads in amber and the session's error
/// follows the note.
private struct ResultNote: View {
    var title: String?
    let text: String
    var error: String?
    let onOpenRaw: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                if let title {
                    Label(title, systemImage: "exclamationmark.triangle")
                        .font(Mono.bold)
                        .foregroundStyle(Theme.running)
                }
                Text(text).foregroundStyle(Theme.dim).fixedSize(horizontal: false, vertical: true)
                if let error, !error.isEmpty {
                    SessionErrorView(error: error)
                }
                // Explicit colour: `.link` style rendered as plain body text here.
                Button(action: onOpenRaw) {
                    Text("→ Open Raw").foregroundStyle(Theme.accent)
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
            .font(Mono.body)
            .textSelection(.enabled)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
