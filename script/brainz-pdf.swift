// Brainz PDF helper. Renders each page of a PDF to a PNG in the given
// directory and prints a JSON list of the files, so the PDF tab can show
// pages as images without a PDF library in the app. CoreGraphics only.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func emit(_ payload: [String: Any]) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: payload)
    FileHandle.standardOutput.write(data)
    exit(0)
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard arguments.count >= 2 else {
    emit(["status": "usage", "pages": []])
}
let pdfPath = arguments[0]
let outDir = URL(fileURLWithPath: arguments[1], isDirectory: true)
let targetWidth = arguments.count > 2 ? (Double(arguments[2]) ?? 1400) : 1400
let maxPages = arguments.count > 3 ? (Int(arguments[3]) ?? 200) : 200

guard let document = CGPDFDocument(URL(fileURLWithPath: pdfPath) as CFURL) else {
    emit(["status": "unreadable", "pages": []])
}
if document.isEncrypted && !document.isUnlocked {
    emit(["status": "encrypted", "pages": []])
}
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

var pages: [[String: Any]] = []
let count = min(document.numberOfPages, maxPages)
for index in 1...max(count, 1) where index <= count {
    guard let page = document.page(at: index) else { continue }
    var box = page.getBoxRect(.cropBox)
    if box.isEmpty { box = page.getBoxRect(.mediaBox) }
    let rotation = page.rotationAngle
    let rotated = rotation % 180 != 0
    let pageWidth = rotated ? box.height : box.width
    let pageHeight = rotated ? box.width : box.height
    guard pageWidth > 0, pageHeight > 0 else { continue }
    let scale = targetWidth / Double(pageWidth)
    let width = Int((Double(pageWidth) * scale).rounded())
    let height = Int((Double(pageHeight) * scale).rounded())
    guard let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { continue }
    context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    context.interpolationQuality = .high
    context.saveGState()
    context.concatenate(page.getDrawingTransform(
        .cropBox, rect: CGRect(x: 0, y: 0, width: width, height: height), rotate: 0, preserveAspectRatio: true))
    context.drawPDFPage(page)
    context.restoreGState()
    guard let image = context.makeImage() else { continue }
    let file = outDir.appendingPathComponent(String(format: "page-%03d.png", index))
    guard let destination = CGImageDestinationCreateWithURL(file as CFURL, UTType.png.identifier as CFString, 1, nil) else { continue }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { continue }
    pages.append(["path": file.path, "width": width, "height": height])
}
emit(["status": "ok", "pages": pages, "total": document.numberOfPages])
