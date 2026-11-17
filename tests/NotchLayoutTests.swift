import SwiftUI

@main struct NotchLayoutTests {
    @MainActor static func main() {
        for count in 1...3 {
            let sessions = (0..<(count == 3 ? 4 : count)).map { i in
                let repo = "repo\(min(i, count - 1))"
                return Session(tool: "claude", id: "\(i)", title: "Test session", project: repo, repo: repo,
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
                let narrow = NotchView(store: store, notch: view.notch, room: 600, open: true, forcedPalette: .named("cool"))
                let narrowImage = ImageRenderer(content: narrow).nsImage!
                assert(usageShape(narrowImage).bar, "Narrow footers should wrap usage items and keep their bars")
            }
            store.limits = []
            let withoutLimits = ImageRenderer(content: view).nsImage!
            assert(image.size.height > withoutLimits.size.height, "Footer is missing for \(count) repos")
            assert(CGFloat(usage.top) >= withoutLimits.size.height - 4, "Limits should appear below the sessions")
        }
        print("Notch layout tests passed")
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
                if run >= 30 { bar = true } // A solid usage bar is longer than any text stroke.
            }
        }
        return (bar, lastY - firstY + 1, firstY)
    }
}
