// mac_window --close-notifications: see below.
// mac_window <pid> [x y w h]: prints "<CGWindowID> x y w h" for the largest on-screen window of <pid>. With a frame, the window is
// first brought to the front, moved and sized (Accessibility, which the shell running this needs). For `screencapture -l <id>`.
import AppKit
import ApplicationServices

let a = CommandLine.arguments

// `mac_window --close-notifications`: closes the notifications on screen (each one's own Close action), so a whole-screen picture shows
// none of the Mac's own alerts.
if a.count == 2 && a[1] == "--close-notifications" {
  guard let nc = NSWorkspace.shared.runningApplications.first(where: { $0.bundleIdentifier == "com.apple.notificationcenterui" }) else { exit(0) }
  func attr(_ e: AXUIElement, _ name: String) -> CFTypeRef? { var v: CFTypeRef?; AXUIElementCopyAttributeValue(e, name as CFString, &v); return v }
  var closed = 0
  func walk(_ e: AXUIElement, _ depth: Int) {
    var names: CFArray?
    AXUIElementCopyActionNames(e, &names)
    if let close = ((names as? [String]) ?? []).first(where: { $0.hasPrefix("Name:Close") }) {
      if AXUIElementPerformAction(e, close as CFString) == .success { closed += 1 }
      return
    }
    for c in (attr(e, kAXChildrenAttribute as String) as? [AXUIElement]) ?? [] where depth < 8 { walk(c, depth + 1) }
  }
  for w in (attr(AXUIElementCreateApplication(nc.processIdentifier), kAXWindowsAttribute as String) as? [AXUIElement]) ?? [] { walk(w, 0) }
  print("closed \(closed)")
  exit(0)
}

guard a.count == 2 || a.count == 6, let pid = Int32(a[1]) else { print("usage: mac_window pid [x y w h]"); exit(2) }
if a.count == 6, let x = Double(a[2]), let y = Double(a[3]), let w = Double(a[4]), let h = Double(a[5]) {
  NSRunningApplication(processIdentifier: pid)?.activate()
  let ax = AXUIElementCreateApplication(pid)
  var wins: CFTypeRef?
  AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &wins)
  guard let win = (wins as? [AXUIElement])?.first else { print("no window (Accessibility?)"); exit(1) }
  var p = CGPoint(x: x, y: y)
  var s = CGSize(width: w, height: h)
  AXUIElementSetAttributeValue(win, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
  AXUIElementSetAttributeValue(win, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &s)!)
  AXUIElementSetAttributeValue(win, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
  Thread.sleep(forTimeInterval: 0.6)
}
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
func bounds(_ w: [String: Any]) -> CGRect {
  guard let d = w[kCGWindowBounds as String] else { return .zero }
  return CGRect(dictionaryRepresentation: d as! CFDictionary) ?? .zero
}
let mine = list.filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
guard let best = mine.max(by: { bounds($0).width * bounds($0).height < bounds($1).width * bounds($1).height }),
      let id = best[kCGWindowNumber as String] as? Int else { print("no window"); exit(1) }
let b = bounds(best)
print("\(id) \(Int(b.minX)) \(Int(b.minY)) \(Int(b.width)) \(Int(b.height))")
