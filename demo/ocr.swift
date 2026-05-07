// TraceMind demo helper — Apple Vision OCR.
// Usage: swift demo/ocr.swift <image_path>   (prints recognized text to stdout)
//
// Uses VNRecognizeTextRequest with .accurate level. Same engine as macOS
// Live Text, no model downloads, no network. Works on macOS 11+.
//
// This file is intentionally a tiny single-script tool — the demo's
// screen-capture path is the simplest possible plumbing on top of macOS
// built-ins. Do not add deps. Do not add features.

import Foundation
import Vision
import AppKit

// ---------- args ----------
guard CommandLine.arguments.count >= 2 else {
    FileHandle.standardError.write(Data("usage: ocr <image_path>\n".utf8))
    exit(2)
}
let path = CommandLine.arguments[1]

// ---------- load image ----------
guard let nsimage = NSImage(contentsOfFile: path),
      let cg = nsimage.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write(Data("ocr: could not load image at \(path)\n".utf8))
    exit(1)
}

// ---------- OCR ----------
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true

let handler = VNImageRequestHandler(cgImage: cg, options: [:])
do {
    try handler.perform([request])
} catch {
    FileHandle.standardError.write(Data("ocr: vision error \(error)\n".utf8))
    exit(1)
}

guard let results = request.results else {
    exit(0)
}

let lines: [String] = results.compactMap { obs in
    obs.topCandidates(1).first?.string
}
print(lines.joined(separator: "\n"))
