// Makes the app icon from the artwork: the macOS rounded square on a
// transparent 1024 canvas, so that macOS shows it as it is in light, dark,
// tinted and clear mode instead of putting it on a grey plate.
//
//   swift scripts/make-icon.swift packaging/macos/AppIcon-artwork.jpg packaging/macos/AppIcon.png packaging/windows/AppIcon.ico
//
// The third path is optional: the same icon for Windows, as PNG images in an .ico.

import AppKit
import CoreGraphics

let arguments = CommandLine.arguments
guard arguments.count == 3 || arguments.count == 4, let artwork = NSImage(contentsOfFile: arguments[1]),
      let source = artwork.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("usage: make-icon.swift <artwork> <icon.png> [icon.ico]\n".data(using: .utf8)!)
    exit(1)
}

let canvas = 1024.0
// Apple's grid: the shape is 824 of 1024, centred, with room for the shadow.
let side = 824.0
let shape = CGRect(x: (canvas - side) / 2, y: (canvas - side) / 2, width: side, height: side)

/// The continuous corner of macOS icons, a superellipse rather than a circle arc.
func squircle(_ rect: CGRect) -> CGPath {
    let path = CGMutablePath()
    let exponent = 5.0
    let (a, b) = (rect.width / 2, rect.height / 2)
    let steps = 720
    for step in 0...steps {
        let angle = Double(step) / Double(steps) * 2 * .pi
        let (c, s) = (cos(angle), sin(angle))
        let x = rect.midX + a * copysign(pow(abs(c), 2 / exponent), c)
        let y = rect.midY + b * copysign(pow(abs(s), 2 / exponent), s)
        step == 0 ? path.move(to: CGPoint(x: x, y: y)) : path.addLine(to: CGPoint(x: x, y: y))
    }
    path.closeSubpath()
    return path
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(data: nil, width: Int(canvas), height: Int(canvas), bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
context.interpolationQuality = .high
let outline = squircle(shape)

// A soft shadow under the shape, as macOS icons have.
context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: CGColor(gray: 0, alpha: 0.45))
context.addPath(outline)
context.setFillColor(CGColor(gray: 0, alpha: 1))
context.fillPath()
context.restoreGState()

// The artwork fills the shape, a little enlarged so that the key carries it.
context.saveGState()
context.addPath(outline)
context.clip()
context.setFillColor(CGColor(gray: 0, alpha: 1))
context.fill(shape)
let zoom = 1.18
let drawn = side * zoom
context.draw(source, in: CGRect(x: shape.midX - drawn / 2, y: shape.midY - drawn / 2, width: drawn, height: drawn))
context.restoreGState()

// A faint rim keeps the black shape apart from a dark Dock.
context.addPath(squircle(shape.insetBy(dx: 1.5, dy: 1.5)))
context.setStrokeColor(CGColor(gray: 1, alpha: 0.14))
context.setLineWidth(3)
context.strokePath()

let image = context.makeImage()!

func png(_ image: CGImage, size: Int) -> Data {
    let scaled = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    scaled.interpolationQuality = .high
    scaled.draw(image, in: CGRect(x: 0, y: 0, width: size, height: size))
    let data = NSMutableData()
    let destination = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, scaled.makeImage()!, nil)
    CGImageDestinationFinalize(destination)
    return data as Data
}

guard (try? png(image, size: 1024).write(to: URL(fileURLWithPath: arguments[2]))) != nil else { exit(1) }

if arguments.count == 4 {
    // Windows keeps the shadow and the transparent margin, as its icons have them too.
    let sizes = [16, 24, 32, 48, 64, 128, 256]
    let images = sizes.map { png(image, size: $0) }
    var ico = Data()
    func append16(_ value: Int) { ico.append(contentsOf: [UInt8(value & 0xff), UInt8(value >> 8 & 0xff)]) }
    func append32(_ value: Int) { append16(value & 0xffff); append16(value >> 16) }
    append16(0); append16(1); append16(sizes.count)
    var offset = 6 + 16 * sizes.count
    for (size, data) in zip(sizes, images) {
        ico.append(UInt8(size == 256 ? 0 : size)); ico.append(UInt8(size == 256 ? 0 : size))
        ico.append(0); ico.append(0)
        append16(1); append16(32)
        append32(data.count); append32(offset)
        offset += data.count
    }
    images.forEach { ico.append($0) }
    guard (try? ico.write(to: URL(fileURLWithPath: arguments[3]))) != nil else { exit(1) }
}
