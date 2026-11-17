import SwiftUI

@main struct NotchLayoutTests {
    @MainActor static func main() {
        for tool in ["claude", "codex", "other"] {
            for tint in [Color.red, .blue] {
                let icon = ProviderIcon(tool: tool).foregroundStyle(tint).frame(width: 28, height: 28)
                let image = ImageRenderer(content: icon).nsImage!
                let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
                var painted = 0
                for y in 0..<bitmap.pixelsHigh {
                    for x in 0..<bitmap.pixelsWide {
                        let pixel = bitmap.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                        if pixel.alphaComponent > 0.5 {
                            painted += 1
                            let channel = tint == .red ? pixel.redComponent : pixel.blueComponent
                            let opposite = tint == .red ? pixel.blueComponent : pixel.redComponent
                            assert(channel > opposite + 0.5, "\(tool) icon lost its status tint")
                        }
                    }
                }
                assert(painted > 30, "\(tool) icon is missing")
            }
        }
        for count in 1...3 {
            let sessions = (0..<(count == 3 ? 4 : count)).map { i in
                let repo = "repo\(min(i, count - 1))"
                return Session(tool: i.isMultiple(of: 2) ? "claude" : "codex", id: "\(i)", title: "Test session", project: repo, repo: repo,
                               cwd: "", status: i.isMultiple(of: 2) ? "busy" : "idle", last_prompt: "continue", idle_secs: 6, summary: nil, tty: nil)
            }
            let store = Store(sample: sessions)
            store.limits = [
                ToolLimits(tool: "Claude", windows: [.init(label: "5h", used: 38, reset: "31m"), .init(label: "weekly", used: 98, reset: "21h")]),
                ToolLimits(tool: "Codex", windows: [.init(label: "weekly", used: 1, reset: "1d 7h")]),
            ]
            let view = NotchView(store: store, notch: CGSize(width: 190, height: 32), open: true, forcedPalette: .named("cool"))
            guard let image = ImageRenderer(content: view).nsImage else { fatalError("Panel did not render") }
            let expected = max(300, CGFloat(count) * 268 + 18) // columns, gaps and panel padding
            assert(abs(image.size.width - expected) < 1, "Usage widened \(count) repos to \(image.size.width)pt")
            let usage = usageShape(image)
            assert(usage.bar, "Usage bars are missing for \(count) repos")
            assert(usage.height < 18, "Usage label, bar and value should share one line")
            if count == 3 {
                let narrow = NotchView(store: store, notch: view.notch, room: 460, open: true, forcedPalette: .named("cool"))
                let narrowImage = ImageRenderer(content: narrow).nsImage!
                assert(usageShape(narrowImage).bar, "Narrow footers should wrap usage items and keep their bars")
                assert(narrowImage.size.height > image.size.height, "Narrow footers should wrap when one row cannot fit")
            }
            store.limits = []
            let withoutLimits = ImageRenderer(content: view).nsImage!
            assert(image.size.height > withoutLimits.size.height, "Footer is missing for \(count) repos")
            if count >= 2 {
                assert(image.size.height - withoutLimits.size.height < 28, "All three limits should fit on one row for \(count) repos")
            }
            assert(CGFloat(usage.top) >= withoutLimits.size.height - 4, "Limits should appear below the sessions")
        }
        checkFolderScrolling()
        print("Notch layout tests passed")
    }

    @MainActor static func checkFolderScrolling() {
        let sessions = (0..<10).map { i in
            Session(tool: "claude", id: "\(i)", title: "Session \(i)", project: "repo", repo: "repo",
                    cwd: "", status: "idle", last_prompt: "continue", idle_secs: 6,
                    summary: i.isMultiple(of: 2) ? "A longer summary that wraps onto a second line in this folder column." : nil,
                    tty: nil, stale: i == 2)
        }
        let view = NotchView(store: Store(sample: sessions), notch: CGSize(width: 190, height: 32), open: true)
        let sizes = [1, 2, 3, 4, 10].map { count in
            ImageRenderer(content: view.columns([("repo", Array(sessions.prefix(count)))])).nsImage!.size
        }
        assert(sizes[0].height < sizes[1].height && sizes[1].height < sizes[2].height,
               "Folders with up to three sessions should hug their rows")
        assert(sizes[2] == sizes[3] && sizes[2] == sizes[4],
               "Extra sessions should scroll inside the three-row viewport")

        _ = NSApplication.shared
        let host = NSHostingView(rootView: view.columns([("repo", sessions), ("other", sessions)]))
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: host.fittingSize),
                              styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        func scrollViews(_ view: NSView) -> [NSScrollView] {
            (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap(scrollViews)
        }
        let scrolls = scrollViews(host)
        assert(scrolls.count == 2, "Each crowded folder should have its own scroll view")
        for scroll in scrolls {
            assert(scroll.documentView!.frame.height > scroll.contentView.bounds.height,
                   "Sessions beyond the third must remain reachable by scrolling")
        }
        let neighborOrigin = scrolls[1].contentView.bounds.origin
        scrolls[0].contentView.scroll(to: NSPoint(x: 0, y: 100))
        assert(scrolls[0].contentView.bounds.origin.y > 0, "The first folder should scroll")
        assert(scrolls[1].contentView.bounds.origin == neighborOrigin, "Scrolling a folder must leave its neighbor still")
    }

    static func usageShape(_ image: NSImage) -> (bar: Bool, height: Int, top: Int) {
        let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
        var bar = false, firstY = bitmap.pixelsHigh, lastY = -1
        for y in 0..<bitmap.pixelsHigh {
            var run = 0
            for x in 0..<bitmap.pixelsWide {
                let color = bitmap.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                let coral = color.alphaComponent > 0.9 && color.redComponent > 0.8 && color.greenComponent < 0.6 && color.blueComponent < 0.6
                if coral { firstY = min(firstY, y); lastY = max(lastY, y) }
                run = coral ? run + 1 : 0
                if run >= 22 { bar = true } // A solid usage bar is longer than any text stroke.
            }
        }
        return (bar, lastY - firstY + 1, firstY)
    }
}
