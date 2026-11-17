// Notch overlay: hover the notch to see active Claude Code / Codex sessions (data from `ccs --json`).
import AppKit
import SwiftUI

struct Session: Decodable, Identifiable {
    let tool, id, title, project, repo, cwd, status, last_prompt: String
    let idle_secs: Int
    let summary, tty: String?
}

let binDir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()

/// Stable color per repo name (String.hashValue is randomized per launch).
func repoColor(_ repo: String) -> Color {
    let h = repo.utf8.reduce(UInt32(5381)) { ($0 &* 33) &+ UInt32($1) }
    return Color(hue: Double(h % 360) / 360, saturation: 0.65, brightness: 0.95)
}

func focus(_ s: Session) {
    // without Ghostty, `tell application "Ghostty"` would pop a "Where is Ghostty?" picker, so do nothing
    guard NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.mitchellh.ghostty") != nil else { return }
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    p.arguments = [binDir.appendingPathComponent("focus.applescript").path, s.tty ?? "", s.cwd]
    try? p.run()
}

final class Store: ObservableObject {
    @Published var sessions: [Session] = []
    @Published var limits: String = ""
    var limitsAt = Date.distantPast
    @Published var finished: Session? // last session whose turn just ended, shown briefly under the notch
    let ccs: String = ProcessInfo.processInfo.environment["CCS_BIN"]
        ?? binDir.appendingPathComponent("ccs").path

    init(sample: [Session], finished: Session? = nil) { sessions = sample; self.finished = finished }

    init() {
        refresh()
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in self?.refresh() }
    }

    func refresh() {
        let ccs = self.ccs
        DispatchQueue.global().async {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: ccs)
            p.arguments = ["--json"]
            let pipe = Pipe()
            p.standardOutput = pipe
            guard (try? p.run()) != nil else { return }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            p.waitUntilExit()
            if Date().timeIntervalSince(self.limitsAt) > 60 {
                self.limitsAt = Date()
                let l = readLimits()
                DispatchQueue.main.async { self.limits = l }
            }
            guard let s = try? JSONDecoder().decode([Session].self, from: data) else { return }
            DispatchQueue.main.async { self.update(s) }
        }
    }

    func update(_ new: [Session]) {
        let wasBusy = Set(sessions.filter { $0.status == "busy" }.map(\.id))
        sessions = new
        guard let done = new.first(where: { wasBusy.contains($0.id) && $0.status != "busy" }) else { return }
        NSSound(named: "Tink")?.play()
        withAnimation(.spring(duration: 0.3)) { finished = done }
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
            if self.finished?.id == done.id { withAnimation(.spring(duration: 0.3)) { self.finished = nil } }
        }
    }
}

/// One line of Claude and Codex limits from hooks/limits.sh (next to this repo's build/ folder).
func readLimits() -> String {
    let p = Process()
    p.executableURL = binDir.deletingLastPathComponent().appendingPathComponent("hooks/limits.sh")
    let pipe = Pipe()
    p.standardOutput = pipe
    guard (try? p.run()) != nil else { return "" }
    let out = pipe.fileHandleForReading.readDataToEndOfFile()
    return String(decoding: out, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
}

func ago(_ s: Int) -> String { s < 60 ? "\(s)s" : s < 3600 ? "\(s / 60)m" : "\(s / 3600)h" }

struct NotchView: View {
    @ObservedObject var store: Store
    let notch: CGSize
    @State var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if open {
                Spacer().frame(height: notch.height - 6)
                if store.sessions.isEmpty {
                    Text("No active sessions").foregroundStyle(.secondary)
                }
                // ccs output is already sorted by repo, so consecutive grouping keeps that order
                // shortcut: no horizontal scrolling, so ~5 repos fill a laptop screen; add a ScrollView if that's not enough.
                let columns = HStack(alignment: .top, spacing: 18) {
                    ForEach(groups(store.sessions), id: \.0) { repo, items in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(repo).font(.system(size: 11, weight: .bold)).foregroundStyle(repoColor(repo))
                            ForEach(items) { s in row(s) }
                        }
                        .frame(width: 250, alignment: .topLeading)
                    }
                }
                if !store.limits.isEmpty {
                    Text(store.limits).font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary)
                }
                ViewThatFits(in: .vertical) {
                    columns
                    ScrollView(.vertical) { columns }.frame(height: 520 - notch.height)
                }
                .frame(maxHeight: 520 - notch.height)
            } else if let f = store.finished {
                Spacer().frame(height: notch.height)
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(repoColor(f.repo))
                    Text("\(f.repo): \(f.title)").font(.system(size: 12, weight: .medium)).lineLimit(1)
                }
                .padding(.horizontal, 14).padding(.bottom, 8)
                .onTapGesture { focus(f) }
            } else {
                // collapsed: one dot per busy session, sitting under the notch
                HStack(spacing: 3) {
                    ForEach(store.sessions.filter { $0.status == "busy" }) { s in
                        Circle().fill(repoColor(s.repo)).frame(width: 4, height: 4)
                    }
                }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom).padding(.bottom, 3)
            }
        }
        .foregroundStyle(.white)
        .padding(.horizontal, open ? 18 : 0)
        .padding(.bottom, open ? 14 : 0)
        .frame(width: open || store.finished != nil ? nil : notch.width,
               height: open || store.finished != nil ? nil : notch.height, alignment: .topLeading)
        .frame(minWidth: open ? 300 : store.finished != nil ? notch.width : 0)
        .background(UnevenRoundedRectangle(bottomLeadingRadius: open ? 18 : 10, bottomTrailingRadius: open ? 18 : 10).fill(.black))
        .contentShape(Rectangle())
        .onHover { h in withAnimation(.spring(duration: 0.25)) { open = h } }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    func groups(_ list: [Session]) -> [(String, [Session])] {
        list.reduce(into: []) { acc, s in
            if acc.last?.0 == s.repo { acc[acc.count - 1].1.append(s) } else { acc.append((s.repo, [s])) }
        }
    }

    func row(_ s: Session) -> some View {
        HStack(alignment: .top, spacing: 8) {
            RoundedRectangle(cornerRadius: 1.5).fill(repoColor(s.repo)).frame(width: 3)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Circle().fill(s.status == "busy" ? .orange : .green).frame(width: 6, height: 6)
                    Text(s.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                }
                Text("\(s.tool) · \(s.project) · \(s.status) \(ago(s.idle_secs))")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
                Text(s.summary ?? s.last_prompt).font(.system(size: 11)).foregroundStyle(.white.opacity(0.8)).lineLimit(2)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .contentShape(Rectangle())
        .onTapGesture { focus(s) }
    }
}

/// Lets the first click on the non-activating panel trigger the tap instead of being swallowed.
final class ClickThroughHost<V: View>: NSHostingView<V> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

@main
struct NotchApp {
    static func main() {
        if let i = CommandLine.arguments.firstIndex(of: "--snapshot"), i + 1 < CommandLine.arguments.count {
            MainActor.assumeIsolated { snapshot(to: CommandLine.arguments[i + 1]) }
            return
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let screen = NSScreen.screens.first { $0.safeAreaInsets.top > 0 } ?? NSScreen.main!
        let notch: CGSize
        if let l = screen.auxiliaryTopLeftArea, let r = screen.auxiliaryTopRightArea {
            notch = CGSize(width: screen.frame.width - l.width - r.width, height: screen.safeAreaInsets.top)
        } else {
            notch = CGSize(width: 200, height: 24) // no notch: fake one at the top center
        }
        let size = CGSize(width: screen.frame.width, height: 600) // full width; transparent areas pass clicks through
        let panel = NSPanel(
            contentRect: NSRect(x: screen.frame.midX - size.width / 2, y: screen.frame.maxY - size.height,
                                width: size.width, height: size.height),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.contentView = ClickThroughHost(rootView: NotchView(store: Store(), notch: notch))
        panel.orderFrontRegardless()
        app.run()
    }
}

/// `notch --snapshot <dir>`: renders README screenshots from sample data (no screen-recording permission needed).
@MainActor func snapshot(to dir: String) {
    func s(_ repo: String, _ title: String, _ status: String, _ idle: Int, _ summary: String, tool: String = "claude") -> Session {
        Session(tool: tool, id: title, title: title, project: repo, repo: repo, cwd: "", status: status,
                last_prompt: "", idle_secs: idle, summary: summary, tty: nil)
    }
    let sample = [
        s("api", "Add rate limiting to /login", "busy", 8, "Writing middleware tests; no input needed yet."),
        s("api", "Migrate users table", "idle", 240, "Migration ready; waiting on you to approve running it."),
        s("web-app", "Dark mode toggle", "idle", 35, "Toggle done and screenshots taken; waiting for review."),
        s("web-app", "Fix flaky checkout test", "busy", 3, "Re-running checkout suite after fixing a race.", tool: "codex"),
        s("infra", "Bump Postgres to 17", "idle", 900, "Opened PR #412; CI green, waiting on merge."),
    ]
    let notch = CGSize(width: 190, height: 32)
    func save(_ view: some View, _ name: String, height: CGFloat) {
        let framed = view.frame(width: 1000, height: height)
            .background(LinearGradient(colors: [Color(red: 0.25, green: 0.3, blue: 0.5), Color(red: 0.55, green: 0.35, blue: 0.5)],
                                       startPoint: .topLeading, endPoint: .bottomTrailing))
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation,
              let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else { return print("render failed: \(name)") }
        try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent(name))
    }
    save(NotchView(store: Store(sample: sample), notch: notch, open: true), "notch-open.png", height: 215)
    save(NotchView(store: Store(sample: sample, finished: sample[2]), notch: notch), "notch-finished.png", height: 78)
}
