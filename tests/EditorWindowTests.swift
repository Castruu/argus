import Foundation

@main struct EditorWindowTests {
    static func main() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let workspace = dir.appendingPathComponent("team project.code-workspace")
        let absolute = dir.appendingPathComponent("shared folder")
        let uri = dir.appendingPathComponent("uri folder")
        let folders: [[String: String]] = [["path": "api"], ["path": absolute.path], ["uri": uri.absoluteString]]
        let json = String(decoding: try JSONSerialization.data(withJSONObject: folders), as: UTF8.self)
        try """
        {
            // Workspace files allow comments and trailing commas.
            "folders": \(json),
        }
        """.write(to: workspace, atomically: true, encoding: .utf8)
        let window: [String: Any] = ["workspaceIdentifier": ["id": "team", "configURIPath": workspace.absoluteString]]
        let state: [String: Any] = ["openedWindows": [window]]
        let api = dir.appendingPathComponent("api").path
        assert(editorWindow(state: state, containing: api) == workspace.path)
        assert(editorWindow(state: state, containing: api + "/src") == workspace.path)
        assert(editorWindow(state: state, containing: absolute.path + "/src") == workspace.path)
        assert(editorWindow(state: state, containing: uri.path) == workspace.path)
        assert(editorWindow(state: state, containing: api + "-other") == nil)
        assert(editorWindow(state: ["lastActiveWindow": window], containing: api) == workspace.path)

        let parent: [String: Any] = ["folder": dir.absoluteString]
        let nested = dir.appendingPathComponent("api/nested")
        let child: [String: Any] = ["folder": nested.absoluteString]
        let mixed: [String: Any] = ["openedWindows": [parent, window, child]]
        assert(editorWindow(state: mixed, containing: api) == workspace.path)
        assert(editorWindow(state: mixed, containing: nested.path + "/src") == nested.path)
        assert(editorWindow(state: ["openedWindows": [parent]], containing: absolute.path) == dir.path)
        assert(editorWindow(state: ["openedWindows": [["folder": "file:///"]]], containing: api) == "/")
        assert(editorWindow(state: ["openedWindows": [["folder": "vscode-remote://ssh-remote+host" + api]]], containing: api) == nil)
        assert(editorWindow(state: [:], containing: api) == nil)

        try "invalid JSON".write(to: workspace, atomically: true, encoding: .utf8)
        assert(editorWindow(state: state, containing: api) == nil)
        try FileManager.default.removeItem(at: workspace)
        assert(editorWindow(state: state, containing: api) == nil)
        print("Editor window matching tests passed")
    }
}
