import Foundation

// Shadows AppKit's sound player so the Store can be checked without playing audio.
final class NSSound {
    static var played: [String] = []
    let name: String
    init?(named name: String) { self.name = name }
    func play() { Self.played.append(name) }
}

@main struct NotificationSoundTests {
    static func main() {
        let defaults = UserDefaults.standard
        let key = "notificationSoundsMuted"
        defaults.removeObject(forKey: key)
        defer { defaults.removeObject(forKey: key) }
        assert(!defaults.bool(forKey: key))

        func session(_ status: String, needs: Bool = false) -> Session {
            var s = Session(tool: "claude", id: "test", title: "Test", project: "repo", repo: "repo",
                            cwd: "", status: status, last_prompt: "", idle_secs: 0, summary: nil, tty: nil)
            s.needs = needs
            return s
        }

        let store = Store(sample: [])
        store.update([session("busy")])
        assert(NSSound.played.isEmpty) // launch stays silent
        store.update([session("idle")])
        store.update([session("idle", needs: true)])
        assert(NSSound.played == ["Tink", "Glass"])

        defaults.set(true, forKey: key)
        for needs in [false, true] {
            store.update([session("busy")])
            store.update([session("idle", needs: needs)])
            assert(store.toasts.last?.session == session("idle", needs: needs))
        }
        assert(NSSound.played == ["Tink", "Glass"])

        defaults.set(false, forKey: key)
        store.update([session("busy")])
        store.update([session("idle")])
        assert(NSSound.played == ["Tink", "Glass", "Tink"])
        print("Notification sound tests passed")
    }
}
