import AppKit
import RivalKit
import SwiftUI

/// A read-only, monospaced `NSTextView` for logs and prompts. SwiftUI `Text`
/// cannot hold a 256 KB tail smoothly; the text view also brings the native
/// find bar (⌘F, ⌘G, ⇧⌘G with highlight).
///
/// - `follow`: after each update, keep the view on the tail.
/// - `onUserScroll(atBottom)`: the user scrolled; nil for views without follow.
/// - Growth that only appends is applied as an append, so selection and the
///   scroll position survive live updates.
struct LogView: NSViewRepresentable {
    let text: String
    /// A dim note shown when `text` is empty ("(empty log)", …).
    var placeholder: String?
    /// The failure block after the log, in the failure colour.
    var trailer: String?
    var follow: Bool
    var onUserScroll: ((Bool) -> Void)?

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.drawsBackground = true
        scroll.backgroundColor = .black
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        guard let tv = scroll.documentView as? NSTextView else { return scroll }
        tv.isEditable = false
        tv.isSelectable = true
        tv.isRichText = false
        tv.usesFindBar = true
        tv.isIncrementalSearchingEnabled = true
        tv.drawsBackground = true
        tv.backgroundColor = .black
        tv.insertionPointColor = LogStyle.fg
        tv.textContainerInset = NSSize(width: 10, height: 8)
        tv.layoutManager?.allowsNonContiguousLayout = true
        tv.selectedTextAttributes = [.backgroundColor: LogStyle.accent, .foregroundColor: NSColor.black]
        tv.textContainer?.widthTracksTextView = true

        let c = context.coordinator
        c.scroll = scroll
        c.textView = tv
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            c, selector: #selector(Coordinator.boundsChanged(_:)),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView
        )
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.onUserScroll = onUserScroll
        c.apply(text: text, placeholder: placeholder, trailer: trailer, follow: follow)
        LogFind.shared.textView = c.textView
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        NotificationCenter.default.removeObserver(coordinator)
    }

    @MainActor
    final class Coordinator: NSObject {
        weak var scroll: NSScrollView?
        weak var textView: NSTextView?
        var onUserScroll: ((Bool) -> Void)?

        private var logText = ""
        private var placeholder: String?
        private var trailer: String?
        private var follow = true
        /// UTF-16 length of the log part of the storage (the trailer follows).
        private var logLength = 0
        private var programmatic = false

        func apply(text: String, placeholder: String?, trailer: String?, follow: Bool) {
            guard let tv = textView, let storage = tv.textStorage else { return }
            let followTurnedOn = follow && !self.follow
            self.follow = follow

            let sameFrame = placeholder == self.placeholder && trailer == self.trailer
            if sameFrame && text == logText {
                if followTurnedOn { scrollToEnd() }
                return
            }

            let origin = scroll?.contentView.bounds.origin
            programmatic = true
            defer { programmatic = false }

            storage.beginEditing()
            if sameFrame, placeholder == nil, !logText.isEmpty, text.hasPrefix(logText) {
                let delta = String(text[logText.endIndex...])
                storage.insert(NSAttributedString(string: delta, attributes: LogStyle.body), at: logLength)
                logLength += (delta as NSString).length
            } else {
                let full = NSMutableAttributedString()
                if text.isEmpty, let placeholder {
                    full.append(NSAttributedString(string: placeholder, attributes: LogStyle.dim))
                } else {
                    full.append(NSAttributedString(string: text, attributes: LogStyle.body))
                }
                logLength = full.length
                if let trailer {
                    full.append(NSAttributedString(string: trailer, attributes: LogStyle.failure))
                }
                storage.setAttributedString(full)
            }
            storage.endEditing()
            logText = text
            self.placeholder = placeholder
            self.trailer = trailer

            if follow {
                scrollToEnd()
            } else if let origin, let clip = scroll?.contentView {
                clip.scroll(to: origin)
                scroll?.reflectScrolledClipView(clip)
            }
        }

        private func scrollToEnd() {
            guard let tv = textView else { return }
            programmatic = true
            defer { programmatic = false }
            tv.scrollRangeToVisible(NSRange(location: tv.textStorage?.length ?? 0, length: 0))
        }

        @objc func boundsChanged(_ note: Notification) {
            guard !programmatic, let onUserScroll, let clip = scroll?.contentView,
                  let doc = scroll?.documentView else { return }
            let atBottom = clip.bounds.maxY >= doc.frame.height - 4
            // Report only changes, so a refresh does not re-render SwiftUI.
            if atBottom != follow {
                follow = atBottom
                onUserScroll(atBottom)
            }
        }
    }
}

/// Text attributes shared by the log views.
@MainActor
enum LogStyle {
    static let font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
    static let fg = NSColor(Theme.fg)
    static let accent = NSColor(Theme.accent)
    static let body: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
    static let dim: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor(Theme.dim)]
    static let failure: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor(Theme.fail)]
}

/// Routes the Find menu items to the visible log view, wherever focus is.
@MainActor
final class LogFind {
    static let shared = LogFind()
    weak var textView: NSTextView?

    func perform(_ action: NSTextFinder.Action) {
        guard let tv = textView, tv.window != nil else { return }
        tv.window?.makeFirstResponder(tv)
        let item = NSMenuItem()
        item.tag = action.rawValue
        tv.performTextFinderAction(item)
    }
}
