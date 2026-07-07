// Render the Preceipts app icon: a One Dark squircle carrying the app's
// own motif — side-by-side diff bars with the accent claw bracketing a
// commented range. Run via Scripts/build_icon.sh; emits a 1024px PNG.
//
//   swift Scripts/generate_icon.swift /tmp/icon-1024.png

import AppKit

let size: CGFloat = 1024

func rgb(_ value: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    NSColor(
        srgbRed: CGFloat((value >> 16) & 0xFF) / 255,
        green: CGFloat((value >> 8) & 0xFF) / 255,
        blue: CGFloat(value & 0xFF) / 255,
        alpha: alpha)
}

let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocusFlipped(true)

// Squircle plate — macOS icon grid: content inset ~100px, radius ~185.
let plate = NSBezierPath(
    roundedRect: NSRect(x: 100, y: 100, width: 824, height: 824),
    xRadius: 185, yRadius: 185)
let gradient = NSGradient(
    starting: rgb(0x2a303c),
    ending: rgb(0x171b22))!
gradient.draw(in: plate, angle: -90)

// Inner hairline for depth.
rgb(0xffffff, 0.06).setStroke()
let inner = NSBezierPath(
    roundedRect: NSRect(x: 106, y: 106, width: 812, height: 812),
    xRadius: 180, yRadius: 180)
inner.lineWidth = 6
inner.stroke()

// Two diff columns of rounded bars. Rows read like a real hunk:
// context / change / change / addition / context.
let barHeight: CGFloat = 58
let rowGap: CGFloat = 44
let top: CGFloat = 278
let leftX: CGFloat = 220
let rightX: CGFloat = 552
let columnWidth: CGFloat = 252

struct Bar {
    let column: Int  // 0 left (old), 1 right (new)
    let row: Int
    let width: CGFloat  // fraction of column
    let color: NSColor
}

let context = rgb(0x4b5263)
let red = rgb(0xe06c75)
let green = rgb(0x98c379)

let bars: [Bar] = [
    Bar(column: 0, row: 0, width: 0.92, color: context),
    Bar(column: 1, row: 0, width: 0.92, color: context),
    Bar(column: 0, row: 1, width: 0.78, color: red),
    Bar(column: 1, row: 1, width: 0.66, color: green),
    Bar(column: 0, row: 2, width: 0.6, color: red),
    Bar(column: 1, row: 2, width: 0.88, color: green),
    Bar(column: 1, row: 3, width: 0.5, color: green),
    Bar(column: 0, row: 4, width: 0.86, color: context),
    Bar(column: 1, row: 4, width: 0.86, color: context),
]

for bar in bars {
    let x = bar.column == 0 ? leftX : rightX
    let y = top + CGFloat(bar.row) * (barHeight + rowGap)
    let rect = NSRect(x: x, y: y, width: columnWidth * bar.width, height: barHeight)
    bar.color.setFill()
    NSBezierPath(roundedRect: rect, xRadius: barHeight / 2, yRadius: barHeight / 2).fill()
}

// The claw: accent bracket hugging the new side's changed rows (1–3).
let accent = rgb(0x61afef)
accent.setFill()
let clawX = rightX - 46
let clawTop = top + 1 * (barHeight + rowGap) - 10
let clawBottom = top + 3 * (barHeight + rowGap) + barHeight + 10
let rail: CGFloat = 14
let nub: CGFloat = 52
NSBezierPath(
    roundedRect: NSRect(x: clawX, y: clawTop, width: rail, height: clawBottom - clawTop),
    xRadius: rail / 2, yRadius: rail / 2
).fill()
NSBezierPath(
    roundedRect: NSRect(x: clawX, y: clawTop, width: nub, height: rail),
    xRadius: rail / 2, yRadius: rail / 2
).fill()
NSBezierPath(
    roundedRect: NSRect(x: clawX, y: clawBottom - rail, width: nub, height: rail),
    xRadius: rail / 2, yRadius: rail / 2
).fill()

image.unlockFocus()

guard let tiff = image.tiffRepresentation,
    let rep = NSBitmapImageRep(data: tiff),
    let png = rep.representation(using: .png, properties: [:])
else {
    fatalError("could not encode icon PNG")
}

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon-1024.png"
try! png.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
