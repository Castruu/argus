import AppKit
import SwiftUI

@MainActor final class SummaryPreview: NSObject, ObservableObject, NSPopoverDelegate {
    @Published private(set) var isShown = false
    let popover = NSPopover()
    weak var host: NSView?
    /// Set while the notch panel is closed: its rows are still hoverable as they fade out, and a card would reopen the panel.
    var suspended = false
    private var anchorID: UUID?
    private var rect = CGRect.zero
    private var text = ""
    private var overRow = false
    private var overCard = false
    private var pending: Task<Void, Never>?

    override init() {
        super.init()
        popover.behavior = .applicationDefined
        popover.animates = false
        popover.appearance = NSAppearance(named: .darkAqua)
        popover.delegate = self
    }

    func enter(_ id: UUID, text: String, rect: CGRect) {
        guard !suspended else { return }
        if anchorID != id { close(); anchorID = id }
        self.text = text
        self.rect = rect
        overRow = true
        pending?.cancel()
        guard !isShown else { return }
        pending = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(200)) } catch { return }
            self?.show()
        }
    }

    func leave(_ id: UUID) {
        guard anchorID == id else { return }
        overRow = false
        closeAfterLeaving()
    }

    func hoverCard(_ inside: Bool) {
        overCard = inside
        closeAfterLeaving()
    }

    private func closeAfterLeaving() {
        pending?.cancel()
        guard !overRow && !overCard else { return }
        // Leave time to cross the small gap between the row and the card.
        pending = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(150)) } catch { return }
            self?.close()
        }
    }

    func update(_ id: UUID, text: String?) {
        guard anchorID == id else { return }
        guard let text, !text.isEmpty else { close(); return }
        self.text = text
        if isShown { show() }
    }

    func move(_ id: UUID, to rect: CGRect) {
        guard anchorID == id else { return }
        let resume = overRow
        let text = self.text
        close()
        if resume { enter(id, text: text, rect: rect) }
    }

    private func show() {
        guard !text.isEmpty, let host, let window = host.window, !rect.isEmpty else { close(); return }
        let anchor = host.isFlipped ? rect : CGRect(x: rect.minX, y: host.bounds.height - rect.maxY,
                                                   width: rect.width, height: rect.height)
        let card = SummaryCard(text: text) { [weak self] in self?.hoverCard($0) }
        let content = NSHostingController(rootView: card)
        popover.contentViewController = content
        popover.contentSize = content.view.fittingSize
        popover.show(relativeTo: anchor, of: host, preferredEdge: .maxX)
        // Keep the card above the notch panel, including when the app is inactive.
        content.view.window?.level = NSWindow.Level(rawValue: window.level.rawValue + 1)
        isShown = popover.isShown
    }

    func remove(_ id: UUID) {
        if anchorID == id { close() }
    }

    func close() {
        pending?.cancel()
        anchorID = nil
        overRow = false
        overCard = false
        popover.close()
        isShown = false
    }

    func popoverDidClose(_ notification: Notification) {
        pending?.cancel()
        anchorID = nil
        overRow = false
        overCard = false
        isShown = false
    }
}

struct SummaryCard: View {
    let text: String
    let hover: (Bool) -> Void

    var body: some View {
        ViewThatFits(in: .vertical) {
            summary
            ThinScroll { summary.padding(.trailing, 8) } // keep the text clear of the thumb
        }
        .frame(width: 340)
        .frame(maxHeight: 280)
        .padding(12)
        .environment(\.colorScheme, .dark)
        .onHover(perform: hover)
    }

    private var summary: some View {
        Text(text).font(.system(size: 12)).foregroundStyle(.white.opacity(0.9))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
    }
}

struct SummaryHoverCard: ViewModifier {
    let text: String?
    let preview: SummaryPreview
    @State private var id = UUID()
    @State private var rect = CGRect.zero

    func body(content: Content) -> some View {
        content
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { frame in
                // Scrolling or relayout must not leave a card attached to an old row position.
                preview.move(id, to: frame)
                rect = frame
            }
            .onHover { inside in
                if inside, let text, !text.isEmpty { preview.enter(id, text: text, rect: rect) }
                else { preview.leave(id) }
            }
            .onChange(of: text) { _, value in preview.update(id, text: value) }
            .onDisappear { preview.remove(id) }
    }
}
