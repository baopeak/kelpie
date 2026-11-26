// sim_tap <fx> <fy>: clicks the booted iPhone simulator's screen at a fraction (0..1) of its width and height.
// The device screen is found as the largest AX group of the Simulator's front window; without one, the window's content is used.
import AppKit
import ApplicationServices

let args = CommandLine.arguments
guard args.count == 3, let fx = Double(args[1]), let fy = Double(args[2]) else { print("usage: sim_tap fx fy"); exit(2) }
guard let app = NSWorkspace.shared.runningApplications.first(where: { $0.bundleIdentifier == "com.apple.iphonesimulator" }) else { print("no Simulator"); exit(1) }
app.activate()
let ax = AXUIElementCreateApplication(app.processIdentifier)
var wins: CFTypeRef?
AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &wins)
guard let win = (wins as? [AXUIElement])?.first else { print("no window (Accessibility?)"); exit(1) }

func frame(_ e: AXUIElement) -> CGRect? {
  var p: CFTypeRef?, s: CFTypeRef?
  AXUIElementCopyAttributeValue(e, kAXPositionAttribute as CFString, &p)
  AXUIElementCopyAttributeValue(e, kAXSizeAttribute as CFString, &s)
  var pt = CGPoint.zero, sz = CGSize.zero
  guard let p, let s, AXValueGetValue(p as! AXValue, .cgPoint, &pt), AXValueGetValue(s as! AXValue, .cgSize, &sz) else { return nil }
  return CGRect(origin: pt, size: sz)
}
func children(_ e: AXUIElement) -> [AXUIElement] {
  var c: CFTypeRef?
  AXUIElementCopyAttributeValue(e, kAXChildrenAttribute as CFString, &c)
  return (c as? [AXUIElement]) ?? []
}
guard let wf = frame(win) else { print("no window frame"); exit(1) }
// The device's screen: the largest descendant (two levels) that is narrower than the window, else the window below its title bar.
var best = CGRect(x: wf.minX, y: wf.minY + 28, width: wf.width, height: wf.height - 28)
var bestArea = 0.0
for c in children(win) + children(win).flatMap(children) {
  if let f = frame(c), f.width < wf.width - 1 || f.height < wf.height - 1, f.width > 100, Double(f.width * f.height) > bestArea {
    bestArea = Double(f.width * f.height); best = f
  }
}
let pt = CGPoint(x: best.minX + best.width * fx, y: best.minY + best.height * fy)
print("window \(wf), screen \(best), click \(pt)")
Thread.sleep(forTimeInterval: 0.3)
for type in [CGEventType.leftMouseDown, .leftMouseUp] {
  CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
  Thread.sleep(forTimeInterval: 0.08)
}
