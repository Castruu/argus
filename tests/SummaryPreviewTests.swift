import AppKit
import SwiftUI

@main struct SummaryPreviewTests {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let wasActive = app.isActive
        let screen = NSScreen.main!
        let bounds = screen.visibleFrame
        let window = NSPanel(contentRect: NSRect(x: bounds.midX, y: bounds.midY, width: 250, height: 80),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        let preview = SummaryPreview()
        let host = NSHostingView(rootView: Color.clear)
        preview.host = host
        window.contentView = host
        let row = UUID()
        let rect = CGRect(x: 0, y: 0, width: 250, height: 80)
        var text = "The complete summary is shown without truncating the final words."
        window.orderFrontRegardless()

        preview.enter(row, text: text, rect: rect)
        wait(0.10)
        assert(!preview.isShown, "Passing over a row should not immediately open the card")
        preview.leave(row)
        wait(0.25)
        assert(!preview.isShown, "Leaving before the delay must cancel presentation")

        preview.enter(row, text: text, rect: rect)
        wait(0.30)
        assert(preview.isShown, "The card should open after 200 ms without a click")
        assert(app.isActive == wasActive, "Showing a summary must not activate the app")
        let cardWindow = preview.popover.contentViewController!.view.window!
        assert(cardWindow.level.rawValue > window.level.rawValue, "The card must stay above the notch panel")
        assert(preview.popover.contentSize.height < 120, "Short summaries should hug their text")

        preview.move(row, to: rect.offsetBy(dx: 0, dy: 5))
        assert(!preview.isShown, "Moving rows should dismiss their old card")
        wait(0.30)
        assert(preview.isShown, "A row that settles under the pointer should regain its card")

        preview.leave(row)
        wait(0.05)
        preview.hoverCard(true)
        wait(0.25)
        assert(preview.isShown, "Moving from the row onto the card must keep it open")
        preview.hoverCard(false)
        preview.enter(row, text: text, rect: rect)
        wait(0.25)
        assert(preview.isShown, "Moving back to the row must keep it open")
        preview.leave(row)
        wait(0.25)
        assert(!preview.isShown, "Leaving both the row and card must close the popup")

        let other = UUID()
        preview.enter(row, text: text, rect: rect)
        wait(0.05)
        preview.enter(other, text: "You: the other session's full prompt", rect: rect)
        preview.leave(row)
        wait(0.30)
        assert(preview.isShown, "An old row's exit must not cancel a newer hover")
        preview.remove(row)
        assert(preview.isShown, "Removing an unrelated row must not close the current card")
        preview.update(other, text: "An updated summary")
        let content = preview.popover.contentViewController as! NSHostingController<SummaryCard>
        assert(content.rootView.text == "An updated summary", "Open summaries must update with the session")
        preview.update(other, text: nil)
        assert(!preview.isShown, "Removing a summary must dismiss the card")

        text = String(repeating: "A long summary that must remain readable by scrolling. ", count: 100)
        preview.enter(row, text: text, rect: rect)
        wait(0.30)
        assert(preview.isShown)
        assert(preview.popover.contentSize.height <= 304, "Long summaries must have a bounded height")
        func scrollViews(_ view: NSView) -> [NSScrollView] {
            (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap(scrollViews)
        }
        let cardHost = preview.popover.contentViewController!.view
        cardHost.layoutSubtreeIfNeeded()
        let scroll = scrollViews(cardHost).first!
        assert(scroll.documentView!.frame.height > scroll.contentView.bounds.height,
               "The full long summary must remain reachable by scrolling")
        preview.remove(row)
        assert(!preview.isShown, "Removing a session must dismiss its card")

        for x in [bounds.minX + 2, bounds.maxX - 252] {
            window.setFrameOrigin(NSPoint(x: x, y: bounds.maxY - 90))
            preview.enter(row, text: text, rect: rect)
            wait(0.30)
            let frame = preview.popover.contentViewController!.view.window!.frame
            assert(frame.minX >= screen.frame.minX && frame.maxX <= screen.frame.maxX,
                   "The card must fit on screen at either edge")
            assert(frame.minY >= screen.frame.minY && frame.maxY <= screen.frame.maxY,
                   "The card must fit vertically near the top edge")
            preview.close()
        }

        preview.enter(row, text: text, rect: rect)
        preview.remove(row)
        wait(0.30)
        assert(!preview.isShown, "Removed rows must not reopen from a pending hover")

        let store = Store(sample: [])
        let overlay = ClickThroughHost(rootView: NotchView(store: store, notch: CGSize(width: 190, height: 32),
                                                          open: true, summaryPreview: preview))
        window.contentView = overlay
        preview.host = overlay
        overlay.layoutSubtreeIfNeeded()
        preview.enter(row, text: text, rect: rect)
        wait(0.30)
        assert(preview.isShown && store.open, "The overlay must stay open while its card is shown")
        preview.close()
        wait(0.10)
        assert(!store.open, "The overlay should close after leaving both it and its card")
        preview.enter(row, text: text, rect: rect)
        wait(0.30)
        assert(!preview.isShown && !store.open, "A row fading out with the closed overlay must not reopen it")
        window.orderOut(nil)
        print("Summary preview tests passed")
    }

    @MainActor static func wait(_ seconds: TimeInterval) {
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    }
}
