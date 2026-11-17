import Foundation

/// Return the open folder or workspace file whose folder most closely contains `cwd`.
func editorWindow(state: [String: Any], containing cwd: String) -> String? {
    var windows = state["openedWindows"] as? [[String: Any]] ?? []
    if let last = state["lastActiveWindow"] as? [String: Any] { windows.append(last) }
    var candidates: [(target: String, folder: String)] = []
    for window in windows {
        if let folder = window["folder"] as? String, let url = URL(string: folder), url.isFileURL {
            candidates.append((url.path, url.standardizedFileURL.path))
        }
        guard let identifier = window["workspaceIdentifier"] as? [String: Any],
              let uri = identifier["configURIPath"] as? String, let workspace = URL(string: uri), workspace.isFileURL,
              let data = try? Data(contentsOf: workspace),
              let json = try? JSONSerialization.jsonObject(with: data, options: .json5Allowed) as? [String: Any],
              let folders = json["folders"] as? [[String: Any]] else { continue }
        for folder in folders {
            let url: URL
            if let path = folder["path"] as? String {
                url = URL(fileURLWithPath: path, relativeTo: workspace.deletingLastPathComponent())
            } else if let uri = folder["uri"] as? String, let file = URL(string: uri), file.isFileURL {
                url = file
            } else { continue }
            candidates.append((workspace.path, url.standardizedFileURL.path))
        }
    }
    let cwd = URL(fileURLWithPath: cwd).standardizedFileURL.path
    return candidates.filter { cwd == $0.folder || cwd.hasPrefix($0.folder + ($0.folder == "/" ? "" : "/")) }
        .max { $0.folder.count < $1.folder.count }?.target
}
