// Notch overlay: hover the notch to see active Claude Code / Codex sessions (data from `ccs --json`).
import AppKit
import SwiftUI

struct Session: Decodable, Identifiable {
    let tool, id, title, project, repo, cwd, status, last_prompt: String
    let idle_secs: Int
    let summary, tty: String?
    // optional so older ccs output still decodes
    var needs, stale, dirty: Bool?
    var branch, waiting_for: String?
    var pr: PR?
}

struct PR: Decodable { let number: Int; let state: String?; let url: String?; let ci: String? }

struct Window: Decodable { let used, resets_at: Double }
struct ToolLimits: Decodable { let five_hour, week: Window? }
struct Limits: Decodable { let claude, codex: ToolLimits? }
struct Output: Decodable { let sessions: [Session]; let limits: Limits? }

/// Known usage windows, e.g. ("Claude wk", window).
func usage(_ l: Limits?) -> [(String, Window)] {
    [("Claude", l?.claude), ("Codex", l?.codex)].flatMap { name, t in
        [("5h", t?.five_hour), ("wk", t?.week)].compactMap { label, w in w.map { ("\(name) \(label)", $0) } }
    }
}

func limitColor(_ used: Double, normal: Color) -> Color { used >= 95 ? .red : used >= 80 ? .orange : normal }

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
    @Published var finished: Session? // last session whose turn just ended, shown briefly under the notch
    @Published var limits: Limits?
    var loaded = false
    var tick = 0
    /// Panel is open. Closed, we poll half as often to save CPU; opening refreshes at once.
    var open = false { didSet { if open && !oldValue { refresh() } } }
    var running = false, again = false
    /// Screen asleep or locked: nothing to show, so do nothing.
    var paused = false { didSet { if oldValue && !paused { refresh() } } }
    var events: FSEventStreamRef?
    let ccs: String = ProcessInfo.processInfo.environment["CCS_BIN"]
        ?? binDir.appendingPathComponent("ccs").path

    init(sample: [Session], finished: Session? = nil, limits: Limits? = nil) { sessions = sample; self.finished = finished; self.limits = limits }

    init() {
        refresh()
        // File events drive updates: Claude rewrites a session file when its status changes, and the hooks
        // write into ~/.cache/ccs. So a turn ending shows up at once and an idle machine does no work.
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        try? FileManager.default.createDirectory(atPath: home + "/.cache/ccs", withIntermediateDirectories: true)
        let dirs = [home + "/.claude/sessions", home + "/.cache/ccs", home + "/.codex/sessions"].filter { FileManager.default.fileExists(atPath: $0) }
        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        events = FSEventStreamCreate(nil, { _, info, _, paths, _, _ in
            // ccs's own cache and the temp files are not news
            let paths = Unmanaged<CFArray>.fromOpaque(paths).takeUnretainedValue() as? [String] ?? []
            guard paths.contains(where: { !$0.hasSuffix("cache.json") && !$0.hasSuffix(".tmp") && !$0.hasSuffix(".stamp") }) else { return }
            Unmanaged<Store>.fromOpaque(info!).takeUnretainedValue().refresh()
        }, &ctx, dirs as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.3,
            UInt32(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagNoDefer))
        if let events {
            FSEventStreamSetDispatchQueue(events, .main)
            FSEventStreamStart(events)
        }
        // The timer only covers what has no file event: the open panel's clocks, a Codex session going idle
        // (10s after its last write), processes that died, and git or PR changes.
        let timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.tick += 1
            let codexBusy = self.sessions.contains { $0.tool == "codex" && $0.status == "busy" }
            if self.tick % (self.open ? 1 : codexBusy ? 2 : 15) == 0 { self.refresh() }
        }
        timer.tolerance = 0.5 // lets macOS batch our wakeups with others
        let pause: [(NotificationCenter, String, Bool)] = [
            (NSWorkspace.shared.notificationCenter, NSWorkspace.screensDidSleepNotification.rawValue, true),
            (NSWorkspace.shared.notificationCenter, NSWorkspace.screensDidWakeNotification.rawValue, false),
            (DistributedNotificationCenter.default(), "com.apple.screenIsLocked", true),
            (DistributedNotificationCenter.default(), "com.apple.screenIsUnlocked", false),
        ]
        for (center, name, value) in pause {
            center.addObserver(forName: Notification.Name(name), object: nil, queue: .main) { [weak self] _ in self?.paused = value }
        }
    }

    func refresh() {
        guard !paused else { return }
        guard !running else { again = true; return } // one ccs at a time; run once more when it is done
        running = true
        let ccs = self.ccs
        DispatchQueue.global().async {
            var out: Output?
            let p = Process()
            p.executableURL = URL(fileURLWithPath: ccs)
            p.arguments = ["--json"]
            let pipe = Pipe()
            p.standardOutput = pipe
            if (try? p.run()) != nil {
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                out = try? JSONDecoder().decode(Output.self, from: data)
            }
            DispatchQueue.main.async {
                self.running = false
                if let out { self.update(out) }
                if self.again { self.again = false; self.refresh() }
            }
        }
    }

    func update(_ out: Output) {
        let new = out.sessions
        let wasBusy = Set(sessions.filter { $0.status == "busy" }.map(\.id))
        let wasNeeding = Set(sessions.filter { $0.needs == true }.map(\.id))
        let first = !loaded
        (sessions, limits, loaded) = (new, out.limits, true)
        guard !first else { return } // no chimes for what was already there at launch
        // "needs you" (Glass) wins over "finished" (Tink)
        let asking = new.first { $0.needs == true && !wasNeeding.contains($0.id) }
        guard let done = asking ?? new.first(where: { wasBusy.contains($0.id) && $0.status != "busy" }) else { return }
        NSSound(named: asking != nil ? "Glass" : "Tink")?.play()
        withAnimation(.spring(duration: 0.3)) { finished = done }
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
            if self.finished?.id == done.id { withAnimation(.spring(duration: 0.3)) { self.finished = nil } }
        }
    }
}

func ago(_ s: Int) -> String { s < 60 ? "\(s)s" : s < 3600 ? "\(s / 60)m" : s < 86400 ? "\(s / 3600)h" : "\(s / 86400)d" }

struct NotchView: View {
    @ObservedObject var store: Store
    let notch: CGSize
    var beside = false // --beside-notch: a small pill right of the notch instead of sitting on it
    var trailing: CGFloat = 0 // beside only: gap between the pill's right edge and the screen's right edge
    var room: CGFloat = 1400 // width available to the open panel
    @State var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if open {
                Spacer().frame(height: notch.height - 6)
                // header: counts by state (doubles as the color legend) on the left, usage on the right
                HStack(spacing: 14) {
                    count("working", .orange) { $0.status == "busy" }
                    count("need you", .pink) { $0.needs == true }
                    count("idle", .green) { $0.status != "busy" && $0.needs != true && $0.stale != true }
                    count("stale", .gray) { $0.stale == true }
                    Spacer(minLength: 24)
                    ForEach(usage(store.limits), id: \.0) { label, w in
                        let c = limitColor(w.used, normal: .white.opacity(0.6))
                        HStack(spacing: 6) {
                            Text(label)
                            ZStack(alignment: .leading) {
                                Capsule().fill(.white.opacity(0.15))
                                Capsule().fill(c).frame(width: 44 * min(w.used, 100) / 100)
                            }.frame(width: 44, height: 4)
                            Text("\(Int(w.used))% · resets in \(ago(max(0, Int(w.resets_at - Date().timeIntervalSince1970))))")
                        }.foregroundStyle(c)
                    }
                }.font(.system(size: 11, weight: .semibold))
                if store.sessions.isEmpty {
                    Text("No active sessions").foregroundStyle(.secondary)
                }
                // ccs output is already sorted by repo, so consecutive grouping keeps that order
                // shortcut: no horizontal scrolling, so ~5 repos fill a laptop screen; add a ScrollView if that's not enough.
                let gs = groups(store.sessions)
                // fixed-width columns; when they do not all fit, scroll sideways
                let wanted = CGFloat(gs.count) * (colW + 18) - 18
                if wanted > room - 36 {
                    ScrollView(.horizontal, showsIndicators: false) { columns(gs) }.frame(width: room - 36)
                    Text("\(gs.count) repos · scroll sideways for more").font(.system(size: 10)).foregroundStyle(.secondary)
                } else {
                    columns(gs)
                }
            } else if let f = store.finished {
                Spacer().frame(height: notch.height)
                HStack(spacing: 6) {
                    Image(systemName: f.needs == true ? "bell.fill" : "checkmark.circle.fill")
                        .foregroundStyle(f.needs == true ? .pink : repoColor(f.repo))
                    Text("\(f.repo): \(f.title)\(f.needs == true ? " needs you" : "")").font(.system(size: 12, weight: .medium)).lineLimit(1)
                }
                .padding(.horizontal, 14).padding(.bottom, 8)
                .onTapGesture { focus(f) }
            } else {
                // collapsed: one dot per busy session, sitting under the notch
                HStack(spacing: 3) {
                    ForEach(store.sessions.filter { $0.status == "busy" || $0.needs == true }) { s in
                        Circle().fill(s.needs == true ? .pink : repoColor(s.repo)).frame(width: 4, height: 4)
                    }
                }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom).padding(.bottom, 3)
                .overlay {
                    // needs-you count, else an icon tinted by the highest usage window
                    // (beside only: on the real notch this spot is hidden behind the hardware)
                    let needing = store.sessions.filter { $0.needs == true }.count
                    if !beside {
                    } else if needing > 0 {
                        Text("\(needing)").font(.system(size: 12, weight: .bold)).foregroundStyle(.pink)
                    } else {
                        Image(systemName: "terminal").font(.system(size: 11))
                            .foregroundStyle(limitColor(usage(store.limits).map(\.1.used).max() ?? 0, normal: .white.opacity(0.6)))
                    }
                }
            }
        }
        .foregroundStyle(.white)
        .padding(.horizontal, open ? 18 : 0)
        .padding(.bottom, open ? 14 : 0)
        .fixedSize(horizontal: open, vertical: false) // open panel is as wide as its columns, not the screen
        .frame(width: open || store.finished != nil ? nil : notch.width,
               height: open || store.finished != nil ? nil : notch.height, alignment: .topLeading)
        .frame(minWidth: open ? 300 : store.finished != nil ? notch.width : 0)
        .background(UnevenRoundedRectangle(bottomLeadingRadius: open ? 18 : 10, bottomTrailingRadius: open ? 18 : 10).fill(.black))
        .contentShape(Rectangle())
        .onHover { h in store.open = h; withAnimation(.spring(duration: 0.25)) { open = h } }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: beside ? .topTrailing : .top)
        .padding(.trailing, beside ? trailing : 0)
    }

    let colW: CGFloat = 250

    func columns(_ gs: [(String, [Session])]) -> some View {
        HStack(alignment: .top, spacing: 18) {
            ForEach(gs, id: \.0) { repo, items in
                VStack(alignment: .leading, spacing: 8) {
                    Text(repo).font(.system(size: 11, weight: .bold)).foregroundStyle(repoColor(repo))
                    ForEach(items) { s in row(s) }
                }
                .frame(width: colW, alignment: .topLeading)
            }
        }
    }

    func groups(_ list: [Session]) -> [(String, [Session])] {
        list.reduce(into: []) { acc, s in
            if acc.last?.0 == s.repo { acc[acc.count - 1].1.append(s) } else { acc.append((s.repo, [s])) }
        }
    }

    @ViewBuilder func count(_ label: String, _ color: Color, _ match: (Session) -> Bool) -> some View {
        let n = store.sessions.filter(match).count
        if n > 0 {
            HStack(spacing: 4) {
                Circle().fill(color).frame(width: 6, height: 6)
                Text("\(n) \(label)").foregroundStyle(.white.opacity(0.75))
            }
        }
    }

    /// "#412 ✓": PR number with its CI result; click opens the PR.
    func prLabel(_ pr: PR) -> some View {
        let (mark, color): (String, Color) = pr.state == "MERGED" ? (" merged", .purple) : pr.state == "CLOSED" ? (" closed", .gray)
            : pr.ci == "fail" ? (" ✗", .red) : pr.ci == "pending" ? (" …", .yellow) : pr.ci == "pass" ? (" ✓", .green) : ("", .white.opacity(0.6))
        return Text("#\(String(pr.number))\(mark)").font(.system(size: 11, weight: .semibold)).foregroundStyle(color).fixedSize()
            .onTapGesture { if let u = pr.url.flatMap({ URL(string: $0) }) { NSWorkspace.shared.open(u) } }
    }

    func row(_ s: Session) -> some View {
        let (word, color): (String, Color) = s.needs == true ? ("needs you", .pink) : s.status == "busy" ? ("working", .orange)
            : s.stale == true ? ("stale", .gray) : ("idle", .green)
        // the folder is only worth showing when it differs from the repo header (git worktrees)
        let place = (s.project == s.repo ? "" : " · \(s.project)") + (s.branch.map { " · \($0)" } ?? "")
        return HStack(alignment: .top, spacing: 8) {
            RoundedRectangle(cornerRadius: 1.5).fill(repoColor(s.repo)).frame(width: 3)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Circle().fill(color).frame(width: 6, height: 6)
                    Text(s.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                }
                // status in its color, then tool and branch; yellow dot = uncommitted changes
                HStack(spacing: 5) {
                    Text("\(Text("\(word) \(ago(s.idle_secs))").foregroundColor(color)) · \(s.tool)\(place)\(Text(s.dirty == true ? " •" : "").foregroundColor(.yellow))")
                        .font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                    if let pr = s.pr { prLabel(pr) }
                }
                if s.stale != true { // stale rows stay compact
                    if let summary = s.summary {
                        Text(summary).font(.system(size: 11)).foregroundStyle(.white.opacity(0.8)).lineLimit(2)
                    } else if !s.last_prompt.isEmpty {
                        Text("You: \(s.last_prompt)").font(.system(size: 11)).italic().foregroundStyle(.white.opacity(0.5)).lineLimit(2)
                    }
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, 4).padding(.trailing, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.pink.opacity(s.needs == true ? 0.16 : 0)))
        .opacity(s.stale == true ? 0.5 : 1)
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
        // --beside-notch: for running next to another notch app. boringNotch's window is 640pt wide, centered
        // and at mainMenu + 3, so sit in a small pill just right of it, one level below, and open leftwards.
        let beside = CommandLine.arguments.contains("--beside-notch")
        let pill = beside ? CGSize(width: 56, height: notch.height) : notch
        let trailing = beside ? screen.frame.width / 2 - 324 - pill.width : 0
        let size = CGSize(width: screen.frame.width, height: 600) // full width; transparent areas pass clicks through
        let panel = NSPanel(
            contentRect: NSRect(x: screen.frame.midX - size.width / 2, y: screen.frame.maxY - size.height,
                                width: size.width, height: size.height),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + (beside ? 2 : 3))
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.contentView = ClickThroughHost(rootView: NotchView(store: Store(), notch: pill, beside: beside, trailing: trailing, room: screen.frame.width - trailing - 16))
        panel.orderFrontRegardless()
        app.run()
    }
}

/// `notch --snapshot <dir>`: renders README screenshots from sample data (no screen-recording permission needed).
@MainActor func snapshot(to dir: String) {
    func s(_ repo: String, _ title: String, _ status: String, _ idle: Int, _ summary: String, tool: String = "claude") -> Session {
        Session(tool: tool, id: title, title: title, project: repo, repo: repo, cwd: "", status: status,
                last_prompt: "can we make this even better?", idle_secs: idle, summary: summary.isEmpty ? nil : summary, tty: nil)
    }
    var sample = [
        s("api", "Add rate limiting to /login", "busy", 8, "Writing middleware tests; no input needed yet."),
        s("api", "Migrate users table", "idle", 240, "Migration ready; waiting on you to approve running it."),
        s("web-app", "Dark mode toggle", "idle", 35, ""),
        s("web-app", "Fix flaky checkout test", "busy", 3, "Re-running checkout suite after fixing a race.", tool: "codex"),
        s("infra", "Bump Postgres to 17", "idle", 900, "Opened PR #412; CI green, waiting on merge."),
    ]
    (sample[0].branch, sample[0].dirty, sample[1].needs, sample[4].stale) = ("rate-limit", true, true, true)
    sample[0].pr = PR(number: 412, state: "OPEN", url: nil, ci: "pass")
    let t = Date().timeIntervalSince1970
    let limits = Limits(claude: ToolLimits(five_hour: Window(used: 84, resets_at: t + 4 * 3600), week: Window(used: 20, resets_at: t + 5 * 86400)), codex: nil)
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
    save(NotchView(store: Store(sample: sample, limits: limits), notch: notch, open: true), "notch-open.png", height: 250)
    save(NotchView(store: Store(sample: sample, finished: sample[2]), notch: notch), "notch-finished.png", height: 78)
}
