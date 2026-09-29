import Foundation

/// One block of a prose answer, for the Result tab's small markdown renderer.
/// Not CommonMark: tables, nested structure, images and HTML stay plain text.
public enum MarkdownBlock: Equatable, Sendable {
    /// A "#" … "######" header; `level` is 1...6.
    case heading(level: Int, text: String)
    /// A "-", "*", "+" or "N." / "N)" list item. `marker` is "•" or the
    /// number with its dot; `indent` counts leading spaces / 2.
    case item(marker: String, text: String, indent: Int)
    /// The lines inside a ``` fence, verbatim. An unclosed fence runs to the end.
    case code(String)
    /// Consecutive other lines, joined by newlines.
    case paragraph(String)
}

/// Splits `text` into blocks. Blank lines end a paragraph and are dropped.
/// A plain line right after a list item (no blank line between) continues
/// that item, as in CommonMark; models wrap long bullets that way.
public func markdownBlocks(_ text: String) -> [MarkdownBlock] {
    var out: [MarkdownBlock] = []
    var para: [Substring] = []
    var code: [Substring]?
    // True while the last block is a list item that a plain line may continue.
    var inItem = false

    func flush() {
        if !para.isEmpty { out.append(.paragraph(para.joined(separator: "\n"))) }
        para = []
    }

    for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
        let trimmed = line.drop { $0 == " " }
        if trimmed.hasPrefix("```") {
            if let body = code {
                out.append(.code(body.joined(separator: "\n")))
                code = nil
            } else {
                flush()
                code = []
                inItem = false
            }
            continue
        }
        if code != nil {
            code?.append(line)
            continue
        }
        if trimmed.isEmpty {
            flush()
            inItem = false
            continue
        }
        if let h = heading(trimmed) {
            flush()
            out.append(h)
            inItem = false
        } else if let item = listItem(line) {
            flush()
            out.append(item)
            inItem = true
        } else if inItem, case .item(let marker, let text, let indent)? = out.last {
            out[out.count - 1] = .item(marker: marker, text: text + " " + trimmed, indent: indent)
        } else {
            para.append(line)
        }
    }
    if let body = code { out.append(.code(body.joined(separator: "\n"))) }
    flush()
    return out
}

private func heading(_ s: Substring) -> MarkdownBlock? {
    let hashes = s.prefix { $0 == "#" }.count
    guard (1...6).contains(hashes) else { return nil }
    let rest = s.dropFirst(hashes)
    guard rest.isEmpty || rest.first == " " else { return nil }
    return .heading(level: hashes, text: rest.trimmingCharacters(in: .whitespaces))
}

private func listItem(_ line: Substring) -> MarkdownBlock? {
    let spaces = line.prefix { $0 == " " }.count
    let s = line.dropFirst(spaces)
    if let c = s.first, "-*+".contains(c), s.dropFirst().first == " " {
        return .item(marker: "•", text: String(s.dropFirst(2)), indent: spaces / 2)
    }
    let digits = s.prefix { $0.isASCII && $0.isNumber }
    guard !digits.isEmpty, digits.count <= 3 else { return nil }
    let rest = s.dropFirst(digits.count)
    guard let p = rest.first, p == "." || p == ")", rest.dropFirst().first == " " else { return nil }
    return .item(marker: digits + ".", text: String(rest.dropFirst(2)), indent: spaces / 2)
}
