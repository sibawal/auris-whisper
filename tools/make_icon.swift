import AppKit

let size: CGFloat = 1024
let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()

guard let ctx = NSGraphicsContext.current?.cgContext else { exit(1) }

// Скруглённый квадрат по сетке Apple: тело иконки с полями
let inset: CGFloat = 96
let body = CGRect(x: inset, y: inset, width: size - inset * 2, height: size - inset * 2)
let radius: CGFloat = body.width * 0.2237
let path = NSBezierPath(roundedRect: body, xRadius: radius, yRadius: radius)
path.addClip()

// Градиент
let gradient = NSGradient(colors: [
    NSColor(calibratedRed: 0.16, green: 0.36, blue: 0.90, alpha: 1),
    NSColor(calibratedRed: 0.45, green: 0.22, blue: 0.85, alpha: 1)
])!
gradient.draw(in: body, angle: -90)

// Мягкий блик сверху
let glow = NSGradient(colors: [NSColor(white: 1, alpha: 0.22), NSColor(white: 1, alpha: 0)])!
glow.draw(in: CGRect(x: body.minX, y: body.midY, width: body.width, height: body.height / 2), angle: -90)

// Звуковая волна
let bars: [CGFloat] = [0.20, 0.38, 0.62, 0.90, 0.55, 0.78, 1.00, 0.72, 0.44, 0.66, 0.30, 0.16]
let barWidth: CGFloat = 42
let gap: CGFloat = 24
let totalWidth = CGFloat(bars.count) * barWidth + CGFloat(bars.count - 1) * gap
var x = body.midX - totalWidth / 2
let maxHeight = body.height * 0.52

NSColor.white.setFill()
for value in bars {
    let h = max(barWidth, maxHeight * value)
    let rect = CGRect(x: x, y: body.midY - h / 2, width: barWidth, height: h)
    NSBezierPath(roundedRect: rect, xRadius: barWidth / 2, yRadius: barWidth / 2).fill()
    x += barWidth + gap
}

image.unlockFocus()

guard let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
try! png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
print("иконка готова")
