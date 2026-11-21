import Foundation

@main struct UsageRefreshTests {
    @MainActor static func main() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let log = dir.appendingPathComponent("calls")
        let argus = dir.appendingPathComponent("argus")
        let snapshot = dir.appendingPathComponent("sessions.json")
        let rows = ["claude", "codex"].map { tool in
            ["tool": tool, "id": "shared", "title": "Test", "project": "r", "repo": "r",
             "cwd": "/", "status": "busy", "last_prompt": "", "idle_secs": 0] as [String: Any]
        }
        try JSONSerialization.data(withJSONObject: rows).write(to: snapshot)
        let script = """
        #!/bin/sh
        printf '%s\\n' "$*" >> "\(log.path)"
        if [ "$1" = "--limits" ]; then
          echo '[{"tool":"Claude","windows":[{"label":"5h","used":0,"reset":null},{"label":"weekly","used":100,"reset":"20h"}]}]'
        else
          cat "\(snapshot.path)"
        fi
        """
        try script.write(to: argus, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: argus.path)
        setenv("ARGUS_BIN", argus.path, 1)
        defer { unsetenv("ARGUS_BIN") }
        let store = Store(sample: [])

        func finishRefresh() {
            let deadline = Date().addingTimeInterval(5)
            while store.running && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01))
            }
            assert(!store.running, "Refresh did not complete")
        }
        func networkRequests() -> Int {
            try! String(contentsOf: log, encoding: .utf8).components(separatedBy: "\n")
                .filter { $0 == "--limits --json --refresh" }.count
        }

        store.refresh()
        store.open = true // Opening during an in-flight local refresh must still fetch live usage.
        finishRefresh()
        assert(networkRequests() == 1)
        assert(store.limits.first?.windows.map(\.used) == [0, 100])

        store.refresh() // Timer/file-event refreshes remain local.
        finishRefresh()
        assert(networkRequests() == 1)

        store.open = false
        store.open = true
        finishRefresh()
        assert(networkRequests() == 2)
        store.open = false
        let calls = try String(contentsOf: log, encoding: .utf8)
        RunLoop.current.run(until: Date().addingTimeInterval(4.5))
        let closedCalls = try String(contentsOf: log, encoding: .utf8)
        assert(closedCalls == calls, "A collapsed busy notch must not poll")

        store.refresh()
        store.finished = store.sessions.first { $0.tool == "codex" }
        store.sessionHook(.end, id: "codex/shared")
        assert(store.sessions.map(\.tool) == ["claude"] && store.finished == nil)
        finishRefresh()
        assert(store.sessions.map(\.tool) == ["claude"], "An in-flight scan must not resurrect a closed session")
        store.sessionHook(.start, id: "codex/shared")
        finishRefresh()
        assert(store.sessions.count == 2, "Resuming a session restores it")

        try Data("[]".utf8).write(to: snapshot) // Force-quit: no hook or file event.
        store.open = true
        finishRefresh()
        assert(store.sessions.isEmpty && networkRequests() == 3)
        try JSONSerialization.data(withJSONObject: rows).write(to: snapshot)
        RunLoop.current.run(until: Date().addingTimeInterval(3))
        finishRefresh()
        assert(store.sessions.count == 2 && networkRequests() == 3, "Open-panel polling stays local")
        store.open = false
        print("Usage refresh tests passed")
    }
}
