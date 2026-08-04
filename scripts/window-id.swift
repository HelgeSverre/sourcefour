// Prints the window number of the Sourcefour window owned by the given pid,
// for capture scripts. Matching by pid keeps stale or unrelated Sourcefour
// windows (a previous capture iteration, the installed app) out of captures.
import CoreGraphics
import Foundation
guard CommandLine.arguments.count > 1, let pid = Int(CommandLine.arguments[1]) else {
    FileHandle.standardError.write(Data("usage: window-id.swift <pid>\n".utf8))
    exit(2)
}
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for entry in list {
    let owner = entry["kCGWindowOwnerPID"] as? Int ?? -1
    let name = entry["kCGWindowName"] as? String ?? ""
    if owner == pid, name == "Sourcefour" {
        print(entry["kCGWindowNumber"] as! Int)
        break
    }
}
