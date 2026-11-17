// Notch overlay: hover the notch to see active Claude Code / Codex sessions (data from `ccs --json`).
import AppKit
import SwiftUI

struct Session: Decodable, Identifiable, Equatable {
    let tool, id, title, project, repo, cwd, status, last_prompt: String
    let idle_secs: Int
    let summary, tty: String?
    // optional so older ccs output still decodes
    var needs, stale, dirty: Bool?
    var branch, waiting_for: String?
    var pr: PR?
    var pr_open_url: String? // supplied by ccs after host, path and PR number validation
    var app: String? // bundle of the terminal or editor the session runs in
}

struct PR: Decodable, Equatable { let number: Int; let state: String?; let ci: String?; var draft: Bool? }

let binDir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
let scriptDir = Bundle.main.bundleURL.pathExtension == "app" ? Bundle.main.resourceURL ?? binDir : binDir

/// Repos on screen, sorted by name; set by the Store on every update.
nonisolated(unsafe) var repoNames: [String] = []

/// Stable hash per repo name (String.hashValue is randomized per launch).
func repoHash(_ repo: String) -> UInt32 { repo.utf8.reduce(UInt32(5381)) { ($0 &* 33) &+ UInt32($1) } }

func hex(_ v: UInt32) -> Color {
    Color(red: Double(v >> 16 & 0xFF) / 255, green: Double(v >> 8 & 0xFF) / 255, blue: Double(v & 0xFF) / 255)
}

/// Every color the panel uses. Pick one with the palette icon in the panel header (click for the next,
/// right-click for the list; remembered), or `--palette <id>` for one run.
struct Palette: Identifiable {
    let id, name: String
    let repo: (String) -> Color
    let working, needs, idle, stale: Color
    let caution: Color // pending checks, uncommitted changes
    let failing: Color // failing checks, usage over 95%
    let warn: Color    // usage over 80%
    let merged: Color
    var pass: Color { idle }

    /// One cool family for the states, plus a single warm color kept for things that are broken.
    /// Repos are spread evenly from green through blue to violet over the repos on screen, so
    /// neighbours never look alike; pastel, because the saturated hues belong to the states.
    static let cool = Palette(id: "cool", name: "Cool",
        repo: { repo in
            let i = Double(repoNames.firstIndex(of: repo) ?? 0)
            return Color(hue: 0.30 + 0.50 * (i + 0.5) / Double(max(repoNames.count, 1)), saturation: 0.38, brightness: 0.97)
        },
        working: Color(hue: 0.57, saturation: 0.72, brightness: 1.00),  // sky blue
        needs: Color(hue: 0.76, saturation: 0.55, brightness: 1.00),    // violet
        idle: Color(hue: 0.40, saturation: 0.62, brightness: 0.86),     // green
        stale: .gray,
        caution: Color(hue: 0.48, saturation: 0.65, brightness: 0.92),  // teal
        failing: Color(hue: 0.01, saturation: 0.62, brightness: 1.00),  // coral
        warn: Color(hue: 0.76, saturation: 0.55, brightness: 1.00),
        merged: Color(hue: 0.48, saturation: 0.65, brightness: 0.92))

    /// Six hand-picked muted tones for repos; amber, rose and sage for the states.
    static let soft = Palette(id: "soft", name: "Soft",
        repo: { [0x88C0D0, 0x81A1C1, 0xB48EAD, 0xA3BE8C, 0xEBCB8B, 0xD08770].map(hex)[Int(repoHash($0) % 360 / 60)] },
        working: hex(0xEBCB8B), needs: hex(0xE06C75), idle: hex(0xA3BE8C), stale: hex(0x6B7385),
        caution: hex(0xEBCB8B), failing: hex(0xE06C75), warn: hex(0xEBCB8B), merged: hex(0xB48EAD))

    /// Repos in white; color only where something needs a look: working, needs you, failing, high usage.
    static let focus = Palette(id: "focus", name: "Focus",
        repo: { _ in .white.opacity(0.7) },
        working: hex(0xF2B45A), needs: hex(0xFF6B8B), idle: .white.opacity(0.45), stale: .white.opacity(0.25),
        caution: hex(0xF2B45A), failing: hex(0xFF6B8B), warn: hex(0xF2B45A), merged: .white.opacity(0.55))

    /// The original: any hue on the wheel per repo, classic traffic-light states.
    static let vivid = Palette(id: "vivid", name: "Vivid",
        repo: { Color(hue: Double(repoHash($0) % 360) / 360, saturation: 0.65, brightness: 0.95) },
        working: .orange, needs: .pink, idle: .green, stale: .gray,
        caution: .yellow, failing: .red, warn: .orange, merged: .purple)

    static let all = [cool, soft, focus, vivid]
    static func named(_ id: String) -> Palette { all.first { $0.id == id } ?? .cool }
}

/// `--palette <id>` wins for this run; otherwise the last pick from the header icon.
let paletteFlag: String? = CommandLine.arguments.firstIndex(of: "--palette")
    .flatMap { CommandLine.arguments.indices.contains($0 + 1) ? CommandLine.arguments[$0 + 1] : nil }

/// Marks something as clickable: open-hand cursor and a faint highlight under the mouse.
struct Clickable: ViewModifier {
    @State private var hover = false
    func body(content: Content) -> some View {
        let lit = content
            .background(RoundedRectangle(cornerRadius: 6).fill(.white.opacity(hover ? 0.09 : 0)))
            .onHover { inside in
                hover = inside
                // set, not push/pop: a row can vanish while hovered, which would leave a pushed cursor stuck.
                // macOS only honors this while the overlay is the active app; the highlight always shows.
                (inside ? NSCursor.openHand : NSCursor.arrow).set()
            }
        if #available(macOS 15, *) { lit.pointerStyle(.grabIdle) } else { lit }
    }
}

/// VS Code and its forks, by app name -> folder under ~/Library/Application Support.
let editors = ["Visual Studio Code": "Code", "Visual Studio Code - Insiders": "Code - Insiders",
               "Cursor": "Cursor", "Windsurf": "Windsurf", "VSCodium": "VSCodium"]

/// The open folder or workspace file that contains `cwd`, read from the editor's own window list.
func editorWindow(support: String, containing cwd: String) -> String? {
    let path = NSHomeDirectory() + "/Library/Application Support/\(support)/User/globalStorage/storage.json"
    guard let data = FileManager.default.contents(atPath: path),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let state = json["windowsState"] as? [String: Any] else { return nil }
    return editorWindow(state: state, containing: cwd)
}

/// Brings the session forward in whatever app runs it.
func focus(_ s: Session) {
    func script(_ file: String) { // these find the exact tab by tty (or working directory)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        p.arguments = [scriptDir.appendingPathComponent(file).path, s.tty ?? "", s.cwd]
        try? p.run()
    }
    guard let app = s.app.map({ URL(fileURLWithPath: $0) }) else {
        // no app found (tmux, Codex): Ghostty by tty or cwd, as before. Without Ghostty,
        // `tell application "Ghostty"` would pop a "Where is Ghostty?" picker, so do nothing.
        if NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.mitchellh.ghostty") != nil { script("focus.applescript") }
        return
    }
    let name = app.deletingPathExtension().lastPathComponent
    if name == "Ghostty" {
        script("focus.applescript")
    } else if name == "Terminal" {
        script("focus-terminal.applescript")
    } else if let support = editors[name], let target = editorWindow(support: support, containing: s.cwd) {
        // Opening an already-open folder or workspace file brings its window forward.
        NSWorkspace.shared.open([URL(fileURLWithPath: target)], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
    } else {
        NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration())
    }
}

final class Store: ObservableObject {
    @Published var sessions: [Session] = []
    @Published var finished: Session? // last session whose turn just ended, shown briefly under the notch
    @Published var limits: [ToolLimits] = []
    var loaded = false
    var tick = 0
    /// Panel is open. Closed, we poll half as often to save CPU; opening refreshes at once.
    var open = false { didSet { if open && !oldValue { refresh(fetchLimits: true) } } }
    var running = false, again = false
    var fetchLimitsPending = false
    /// Screen asleep or locked: nothing to show, so do nothing.
    var paused = false { didSet { if oldValue && !paused { refresh() } } }
    var events: FSEventStreamRef?
    let ccs: String = ProcessInfo.processInfo.environment["CCS_BIN"]
        ?? binDir.appendingPathComponent("ccs").path

    init(sample: [Session], finished: Session? = nil) { sessions = sample; self.finished = finished; repoNames = Set(sample.map(\.repo)).sorted() }

    init() {
        refresh()
        // File events drive updates: Claude rewrites a session file when its status changes, and the hooks
        // write into ~/.cache/ccs. So a turn ending shows up at once and an idle machine does no work.
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        try? FileManager.default.createDirectory(atPath: home + "/.cache/ccs", withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
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

    func refresh(fetchLimits: Bool = false) {
        fetchLimitsPending = fetchLimitsPending || fetchLimits
        guard !paused else { return }
        guard !running else { again = true; return } // one ccs at a time; run once more when it is done
        running = true
        let ccs = self.ccs
        let limitsArgs = ["--limits", "--json"] + (fetchLimitsPending ? ["--refresh"] : [])
        fetchLimitsPending = false
        DispatchQueue.global().async {
            let sessions = try? JSONDecoder().decode([Session].self, from: run(ccs, ["--json"]))
            let limits = try? JSONDecoder().decode([ToolLimits].self, from: run(ccs, limitsArgs))
            DispatchQueue.main.async {
                self.running = false
                if let limits, self.limits != limits { self.limits = limits }
                if let sessions { self.update(sessions) }
                if self.again { self.again = false; self.refresh() }
            }
        }
    }

    func update(_ new: [Session]) {
        guard !loaded || sessions != new else { return }
        let wasBusy = Set(sessions.filter { $0.status == "busy" }.map(\.id))
        let wasNeeding = Set(sessions.filter { $0.needs == true }.map(\.id))
        let first = !loaded
        (sessions, loaded) = (new, true)
        repoNames = Set(new.map(\.repo)).sorted()
        guard !first else { return } // no chimes for what was already there at launch
        // "needs you" (Glass) wins over "finished" (Tink)
        let asking = new.first { $0.needs == true && !wasNeeding.contains($0.id) }
        guard let done = asking ?? new.first(where: { wasBusy.contains($0.id) && $0.status == "idle" }) else { return }
        if !UserDefaults.standard.bool(forKey: "notificationSoundsMuted") {
            NSSound(named: asking != nil ? "Glass" : "Tink")?.play()
        }
        withAnimation(.spring(duration: 0.3)) { finished = done }
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
            if self.finished?.id == done.id { withAnimation(.spring(duration: 0.3)) { self.finished = nil } }
        }
    }
}

/// `ccs --limits --json`: per tool, each usage window (5h, weekly, ...).
struct ToolLimits: Decodable, Equatable {
    struct Window: Decodable, Equatable {
        let label: String
        let used: Double
        let reset: String?
    }
    let tool: String
    let windows: [Window]
}

func run(_ bin: String, _ args: [String]) -> Data {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: bin)
    p.arguments = args
    let pipe = Pipe()
    p.standardOutput = pipe
    guard (try? p.run()) != nil else { return Data() }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return data
}

func ago(_ s: Int) -> String { s < 60 ? "\(s)s" : s < 3600 ? "\(s / 60)m" : s < 86400 ? "\(s / 3600)h" : "\(s / 86400)d" }

struct NotchView: View {
    @ObservedObject var store: Store
    let notch: CGSize
    var beside = false // --beside-notch: a small pill right of the notch instead of sitting on it
    var trailing: CGFloat = 0 // beside only: gap between the pill's right edge and the screen's right edge
    var room: CGFloat = 1400 // width available to the open panel
    @State var open = false
    @AppStorage("palette") private var paletteID = "cool"
    @AppStorage("notificationSoundsMuted") private var notificationSoundsMuted = false
    var forcedPalette: Palette? = nil // snapshots
    var p: Palette { forcedPalette ?? Palette.named(paletteFlag ?? paletteID) }

    var body: some View {
        let shape = UnevenRoundedRectangle(bottomLeadingRadius: open ? 18 : 10, bottomTrailingRadius: open ? 18 : 10)
        VStack(alignment: .leading, spacing: 10) {
            if open {
                Spacer().frame(height: notch.height - 6)
                let repos = groups(store.sessions)
                let contentWidth = max(264, min(room - 36, CGFloat(repos.count) * (colW + 18) - 18))
                header.frame(width: contentWidth)
                if store.sessions.isEmpty {
                    Text("No active sessions").foregroundStyle(.secondary)
                }
                // ccs output is already sorted by repo, so consecutive grouping keeps that order
                let cols = columns(repos)
                let maxW = room - 36
                if CGFloat(repos.count) * (colW + 18) - 18 > maxW {
                    ScrollView(.horizontal, showsIndicators: false) { cols }
                        .frame(width: maxW)
                    Text("scroll sideways for more repos").font(.system(size: 10)).foregroundStyle(.secondary)
                } else {
                    cols
                }
                if !store.limits.isEmpty {
                    limitsFooter.frame(width: contentWidth)
                }
            } else if let f = store.finished {
                Spacer().frame(height: notch.height)
                HStack(spacing: 6) {
                    Image(systemName: f.needs == true ? "bell.fill" : "checkmark.circle.fill")
                        .foregroundStyle(f.needs == true ? p.needs : p.repo(f.repo))
                    Text("\(f.repo): \(f.title)\(f.needs == true ? " needs you" : "")").font(.system(size: 12, weight: .medium)).lineLimit(1)
                }
                .padding(.horizontal, 14).padding(.bottom, 8)
                .modifier(Clickable())
                .onTapGesture { focus(f) }
            } else {
                // collapsed: one dot per busy session, sitting under the notch
                HStack(spacing: 3) {
                    ForEach(store.sessions.filter { $0.status == "busy" || $0.needs == true }) { s in
                        Circle().fill(s.needs == true ? p.needs : p.repo(s.repo)).frame(width: 4, height: 4)
                    }
                }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom).padding(.bottom, 3)
                .overlay {
                    // needs-you count, else an icon tinted by the highest usage window
                    // (beside only: on the real notch this spot is hidden behind the hardware)
                    let needing = store.sessions.filter { $0.needs == true }.count
                    if !beside {
                    } else if needing > 0 {
                        Text("\(needing)").font(.system(size: 12, weight: .bold)).foregroundStyle(p.needs)
                    } else {
                        let peak = store.limits.flatMap(\.windows).map(\.used).max() ?? 0
                        Image(systemName: "terminal").font(.system(size: 11))
                            .foregroundStyle(peak >= 95 ? p.failing : peak >= 80 ? p.warn : .white.opacity(0.6))
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
        .background(shape.fill(.black))
        .clipShape(shape)
        .contentShape(Rectangle())
        .onHover { h in store.open = h; withAnimation(.spring(duration: 0.25)) { open = h } }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: beside ? .topTrailing : .top)
        .padding(.trailing, beside ? trailing : 0)
    }

    var header: some View {
        HStack(spacing: 14) {
            count("working", p.working) { $0.status == "busy" }
            count("need you", p.needs) { $0.needs == true }
            count("idle", p.idle) { $0.status != "busy" && $0.needs != true && $0.stale != true }
            count("stale", p.stale) { $0.stale == true }
            Spacer(minLength: 24)
            paletteMenu
            soundToggle
        }.font(.system(size: 11, weight: .semibold))
    }

    var limitsFooter: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { limitItems }
                .fixedSize(horizontal: true, vertical: false)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: colW), spacing: 18, alignment: .leading)], alignment: .leading, spacing: 8) {
                limitItems
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    var limitItems: some View {
        ForEach(store.limits.flatMap { t in t.windows.map { ("\(t.tool) \($0.label)", $0) } }, id: \.0) { label, w in
            let c = w.used >= 95 ? p.failing : w.used >= 80 ? p.warn : .white.opacity(0.6)
            HStack(spacing: 4) {
                Text(label)
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.15))
                    Capsule().fill(c).frame(width: 24 * min(w.used, 100) / 100)
                }.frame(width: 24, height: 4)
                Text("\(Int(w.used.rounded()))%" + (w.reset.map { " ↻\($0)" } ?? "")).monospacedDigit()
            }
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(c)
        }
    }

    /// Header icon: click for the next palette, right-click to pick one. Remembered across launches.
    var paletteMenu: some View {
        Image(systemName: "paintpalette").font(.system(size: 11)).foregroundStyle(.white.opacity(0.5))
            .help("Colors: \(p.name). Click for the next, right-click to choose")
            .modifier(Clickable())
            .onTapGesture {
                let all = Palette.all
                paletteID = all[((all.firstIndex { $0.id == p.id } ?? 0) + 1) % all.count].id
            }
            .contextMenu {
                ForEach(Palette.all) { q in
                    Button { paletteID = q.id } label: {
                        if q.id == p.id { Label(q.name, systemImage: "checkmark") } else { Text(q.name) }
                    }
                }
            }
    }

    var soundToggle: some View {
        Button { notificationSoundsMuted.toggle() } label: {
            Image(systemName: notificationSoundsMuted ? "speaker.slash" : "speaker.wave.2")
                .font(.system(size: 11)).foregroundStyle(.white.opacity(0.5))
        }
        .buttonStyle(.plain)
        .help(notificationSoundsMuted ? "Unmute notification sounds" : "Mute notification sounds")
        .accessibilityLabel(notificationSoundsMuted ? "Unmute notification sounds" : "Mute notification sounds")
        .modifier(Clickable())
    }

    let colW: CGFloat = 250

    func columns(_ gs: [(String, [Session])]) -> some View {
        HStack(alignment: .top, spacing: 18) {
            ForEach(gs, id: \.0) { repo, items in
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 4) {
                        Text(repo).foregroundStyle(p.repo(repo))
                        if items.count > 3 {
                            Text("(\(items.count))").foregroundStyle(.secondary)
                                .help("Scroll for more sessions")
                        }
                    }.font(.system(size: 11, weight: .bold))
                    let rows = VStack(alignment: .leading, spacing: 8) {
                        ForEach(items) { s in row(s) }
                    }
                    if items.count > 3 {
                        // Size the viewport to three real rows, including wrapped summaries.
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(items.prefix(3)) { s in row(s) }
                        }
                        .hidden()
                        .overlay {
                            ScrollView(.vertical) { rows }
                        }
                    } else {
                        rows
                    }
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

    /// "#412 ✓" / "#13 draft ✗": PR number with its CI result; click opens the PR.
    @ViewBuilder func prLabel(_ pr: PR, openURL: String?) -> some View {
        let (mark, color): (String, Color) = pr.state == "MERGED" ? (" merged", p.merged) : pr.state == "CLOSED" ? (" closed", p.stale)
            : pr.ci == "fail" ? (" ✗", p.failing) : pr.ci == "pending" ? (" …", p.caution) : pr.ci == "pass" ? (" ✓", p.idle) : ("", .white.opacity(0.6))
        let draft = pr.draft == true && pr.state == "OPEN" ? " draft" : ""
        let label = Text("#\(String(pr.number))\(draft)\(mark)").font(.system(size: 11, weight: .semibold)).foregroundStyle(color).fixedSize()
        if let url = openURL.flatMap(URL.init(string:)), url.scheme == "https" {
            label.modifier(Clickable()).onTapGesture { NSWorkspace.shared.open(url) }
        } else {
            label
        }
    }

    func row(_ s: Session) -> some View {
        let (word, color): (String, Color) = s.needs == true ? ("needs you", p.needs) : s.status == "busy" ? ("working", p.working)
            : s.stale == true ? ("stale", p.stale) : ("idle", p.idle)
        // the folder is only worth showing when it differs from the repo header (git worktrees)
        let place = (s.project == s.repo ? "" : " · \(s.project)") + (s.branch.map { " · \($0)" } ?? "")
        return HStack(alignment: .top, spacing: 8) {
            RoundedRectangle(cornerRadius: 1.5).fill(p.repo(s.repo).opacity(p.id == "focus" ? 0.35 : 1)).frame(width: 3)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    ProviderIcon(tool: s.tool).foregroundStyle(color).frame(width: 14, height: 14)
                        .help("\(s.tool.capitalized): \(word)")
                    Text(s.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                }
                // status in its color, then worktree and branch; colored dot = uncommitted changes
                HStack(spacing: 5) {
                    Text("\(Text("\(word) \(ago(s.idle_secs))").foregroundColor(color))\(place)\(Text(s.dirty == true ? " •" : "").foregroundColor(p.caution))")
                        .font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                    if let pr = s.pr { prLabel(pr, openURL: s.pr_open_url) }
                }
                if s.stale != true { // stale rows stay compact
                    if let summary = s.summary {
                        Text(summary).font(.system(size: 11)).foregroundStyle(.white.opacity(0.8)).lineLimit(2)
                            .help(summary)
                    } else if !s.last_prompt.isEmpty {
                        Text("You: \(s.last_prompt)").font(.system(size: 11)).italic().foregroundStyle(.white.opacity(0.5)).lineLimit(2)
                            .help("You: \(s.last_prompt)")
                    }
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, 4).padding(.trailing, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 6).fill(p.needs.opacity(s.needs == true ? 0.16 : 0)))
        .opacity(s.stale == true ? 0.5 : 1)
        .contentShape(Rectangle())
        .modifier(Clickable())
        .onTapGesture { focus(s) }
    }
}

/// Lets the first click on the non-activating panel trigger the tap instead of being swallowed.
final class ClickThroughHost<V: View>: NSHostingView<V> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

#if !NOTCH_TESTS
@main
#endif
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
    sample[0].pr = PR(number: 412, state: "OPEN", ci: "pass")
    (sample[3].branch, sample[3].pr) = ("fix-checkout", PR(number: 87, state: "OPEN", ci: "fail", draft: true))
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
    let withLimits = Store(sample: sample)
    withLimits.limits = [
        ToolLimits(tool: "Claude", windows: [.init(label: "5h", used: 16, reset: "3h"), .init(label: "weekly", used: 86, reset: "23h")]),
        ToolLimits(tool: "Codex", windows: [.init(label: "weekly", used: 12, reset: "4d")]),
    ]
    save(NotchView(store: withLimits, notch: notch, open: true), "notch-open.png", height: 270)
    if CommandLine.arguments.contains("--palettes") { // every palette on one sheet, for comparing
        let sheet = VStack(spacing: 0) {
            ForEach(Palette.all) { q in
                VStack(alignment: .leading, spacing: 4) {
                    Text(q.name).font(.system(size: 15, weight: .bold)).foregroundStyle(.white).padding(.leading, 16)
                    NotchView(store: withLimits, notch: notch, open: true, forcedPalette: q).frame(width: 1000, height: 270)
                }.padding(.top, 10)
            }
        }
        save(sheet, "palettes.png", height: CGFloat(Palette.all.count) * 304)
    }
    save(NotchView(store: Store(sample: sample, finished: sample[2]), notch: notch), "notch-finished.png", height: 78)
}
