import Foundation

/// How much of a log's tail the app reads (`logfmt.MaxTailBytes`). Logs reach
/// tens of megabytes; earlier output stays reachable through "Open full log".
public let maxTailBytes = 256 << 10

/// How many spaces one tab becomes (`logfmt.TabWidth`).
public let logTabWidth = 4

/// Makes a raw CLI log safe to display, line by line (`logfmt.Sanitize` plus
/// `logfmt.ExpandTabs`): keeps only the last carriage-return progress frame,
/// strips ANSI escape sequences, drops C0 control characters and DEL, and
/// expands each tab to `logTabWidth` spaces. Newlines are preserved.
public func sanitizeLog(_ raw: String) -> String {
    expandTabs(sanitizeKeepingTabs(raw), width: logTabWidth)
}

/// `logfmt.Sanitize`: like `sanitizeLog`, but tabs survive.
public func sanitizeKeepingTabs(_ raw: String) -> String {
    // Split on scalars, not Characters: "\r\n" is one Character and would
    // never match a "\n" separator.
    raw.unicodeScalars.split(separator: "\n", omittingEmptySubsequences: false)
        .map(sanitizeLine)
        .joined(separator: "\n")
}

/// `logfmt.ExpandTabs`. A non-positive width removes tabs.
public func expandTabs(_ s: String, width: Int) -> String {
    s.replacingOccurrences(of: "\t", with: String(repeating: " ", count: max(0, width)))
}

/// `logfmt.SanitizeLine`. The order matters: the trailing "\r" of a CRLF line
/// is trimmed first, or the progress-frame rule would blank every such line.
func sanitizeLine(_ line: Substring.UnicodeScalarView) -> String {
    var scalars = line
    if scalars.last == "\r" { scalars.removeLast() }
    if let cr = scalars.lastIndex(of: "\r") { scalars = scalars[scalars.index(after: cr)...] }

    var out = String.UnicodeScalarView()
    for scalar in stripANSI(scalars) {
        let v = scalar.value
        if v == 0x09 || !(v < 0x20 || v == 0x7F) { out.append(scalar) }
    }
    return String(out)
}

/// Removes ANSI escape sequences, following the DEC VT parser states that
/// `x/ansi.Strip` uses: CSI (`ESC [` … final byte), OSC (`ESC ]` … BEL or ST),
/// DCS/SOS/PM/APC strings (… ST), and two-byte escapes with optional
/// intermediates. CAN and SUB abort a sequence. C0 controls outside sequences
/// are kept here and dropped by the caller.
func stripANSI(_ input: Substring.UnicodeScalarView) -> [Unicode.Scalar] {
    enum State { case ground, escape, escIntermediate, csi, oscString, termString }
    var state = State.ground
    var out: [Unicode.Scalar] = []
    out.reserveCapacity(input.count)
    for scalar in input {
        let v = scalar.value
        if v == 0x1B {
            state = .escape
            continue
        }
        if v == 0x18 || v == 0x1A {
            if state != .ground { state = .ground; continue }
        }
        switch state {
        case .ground:
            out.append(scalar)
        case .escape:
            switch v {
            case 0x5B: state = .csi // [
            case 0x5D: state = .oscString // ]
            case 0x50, 0x58, 0x5E, 0x5F: state = .termString // P X ^ _
            case 0x20...0x2F: state = .escIntermediate
            case 0x30...0x7E: state = .ground // includes "\" (ST)
            default: break
            }
        case .escIntermediate:
            if (0x30...0x7E).contains(v) { state = .ground }
        case .csi:
            if (0x40...0x7E).contains(v) { state = .ground }
        case .oscString:
            if v == 0x07 { state = .ground }
        case .termString:
            break
        }
    }
    return out
}

/// Reads up to `maxBytes` from the end of the file at `path`, reporting whether
/// earlier output was cut (`logfmt.ReadTail`). A tail that starts mid-file is
/// aligned past its first newline so no rune or escape is cut in half. Invalid
/// UTF-8 is dropped. The text is raw; pass it through `sanitizeLog`.
public func readTail(path: String, maxBytes: Int) throws -> (text: String, truncated: Bool) {
    let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
    defer { try? handle.close() }
    let size = try handle.seekToEnd()
    let truncated = size > UInt64(maxBytes)
    try handle.seek(toOffset: truncated ? size - UInt64(maxBytes) : 0)
    var data = try handle.read(upToCount: maxBytes) ?? Data()
    if truncated, let nl = data.firstIndex(of: 0x0A), nl + 1 < data.endIndex {
        data = data[(nl + 1)...]
    }
    return (dropInvalidUTF8(data), truncated)
}

/// Decodes UTF-8 and drops invalid byte sequences without a replacement.
func dropInvalidUTF8(_ data: Data) -> String {
    var it = data.makeIterator()
    var decoder = UTF8()
    var out = String.UnicodeScalarView()
    loop: while true {
        switch decoder.decode(&it) {
        case .scalarValue(let s): out.append(s)
        case .error: continue
        case .emptyInput: break loop
        }
    }
    return String(out)
}
