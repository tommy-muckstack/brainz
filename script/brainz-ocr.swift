// Brainz OCR helper. Reads an image file and prints the recognized text as
// JSON so the composer can log an email screenshot without the agent
// reading pixels. Plain Vision framework, accurate mode, on device.
import Foundation
import Vision

func emit(_ payload: [String: Any]) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: payload)
    FileHandle.standardOutput.write(data)
    exit(0)
}

guard let path = CommandLine.arguments.dropFirst().first else {
    emit(["status": "usage", "text": ""])
}
let url = URL(fileURLWithPath: path)
guard let data = try? Data(contentsOf: url) else {
    emit(["status": "unreadable", "text": ""])
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = ["en-US"]
let handler = VNImageRequestHandler(data: data, options: [:])
do {
    try handler.perform([request])
} catch {
    emit(["status": "failed", "error": "\(error)", "text": ""])
}
let observations = (request.results ?? []) as [VNRecognizedTextObservation]
// Vision returns boxes bottom-up; sort top-to-bottom, then left-to-right, so
// the text reads like the screenshot.
let sorted = observations.sorted { a, b in
    let rowA = (a.boundingBox.midY * 100).rounded()
    let rowB = (b.boundingBox.midY * 100).rounded()
    if abs(rowA - rowB) > 1 { return rowA > rowB }
    return a.boundingBox.minX < b.boundingBox.minX
}
let lines = sorted.compactMap { $0.topCandidates(1).first?.string }
emit(["status": "ok", "text": lines.joined(separator: "\n"), "lines": lines])
