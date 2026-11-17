import SwiftUI

@main struct NotchLayoutTests {
    @MainActor static func main() {
        for count in 1...2 {
            let sessions = (0..<count).map { i in
                Session(tool: "claude", id: "\(i)", title: "Test session", project: "repo\(i)", repo: "repo\(i)",
                        cwd: "", status: "idle", last_prompt: "continue", idle_secs: 6, summary: nil, tty: nil)
            }
            let store = Store(sample: sessions)
            store.limits = [
                ToolLimits(tool: "Claude", windows: [.init(label: "5h", used: 38, reset: "31m"), .init(label: "weekly", used: 98, reset: "21h")]),
                ToolLimits(tool: "Codex", windows: [.init(label: "weekly", used: 1, reset: "1d 7h")]),
            ]
            let view = NotchView(store: store, notch: CGSize(width: 190, height: 32), open: true)
            guard let image = ImageRenderer(content: view).nsImage else { fatalError("Panel did not render") }
            let expected: CGFloat = count == 1 ? 300 : 554 // columns, gaps and panel padding
            assert(abs(image.size.width - expected) < 1, "Usage widened \(count) repos to \(image.size.width)pt")
        }
        print("Notch layout tests passed")
    }
}
