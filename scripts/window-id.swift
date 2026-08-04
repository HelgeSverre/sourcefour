// Prints the window number of the app's main window, for capture scripts.
import CoreGraphics
import Foundation
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for entry in list {
    let owner = (entry["kCGWindowOwnerName"] as? String ?? "").lowercased()
    let name = entry["kCGWindowName"] as? String ?? ""
    if owner.contains("sourcefour"), name == "Sourcefour" {
        print(entry["kCGWindowNumber"] as! Int)
        break
    }
}
