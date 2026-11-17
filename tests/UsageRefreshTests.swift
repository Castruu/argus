import Foundation

@main struct UsageRefreshTests {
    @MainActor static func main() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let log = dir.appendingPathComponent("calls")
        let ccs = dir.appendingPathComponent("ccs")
        let script = """
        #!/bin/sh
        printf '%s\\n' "$*" >> "\(log.path)"
        if [ "$1" = "--limits" ]; then
          echo '[{"tool":"Claude","windows":[{"label":"5h","used":0,"reset":null},{"label":"weekly","used":100,"reset":"20h"}]}]'
        else
          echo '[]'
        fi
        """
        try script.write(to: ccs, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: ccs.path)
        setenv("CCS_BIN", ccs.path, 1)
        defer { unsetenv("CCS_BIN") }
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
        print("Usage refresh tests passed")
    }
}
