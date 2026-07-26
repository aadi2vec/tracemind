// tm-capture — macOS Vision framework OCR shim.
//
// Called from Rust via `swift vision_ocr.swift <png-path>`; prints the
// recognized text to stdout, one line per Vision text observation, or
// exits non-zero on failure. Kept intentionally small — no CLI parser,
// no logging, no threading. The Rust caller is responsible for path
// validation and timeout.
//
// Vision's recognitionLevel is `.accurate` (slower but higher-quality
// than `.fast`) since screenshot capture is asynchronous — we care
// more about a clean whiteboard transcription than millisecond turn-
// around. usesLanguageCorrection stays on for the same reason.
//
// Language hint: English + auto-detect. Users capturing non-English
// text will still get recognized text; Vision's language detector
// handles the common cases.

import Foundation
import Vision
import CoreImage
import AppKit

guard CommandLine.arguments.count >= 2 else {
    FileHandle.standardError.write("usage: vision_ocr <png-path>\n".data(using: .utf8)!)
    exit(2)
}

let path = CommandLine.arguments[1]
guard let image = NSImage(contentsOfFile: path),
      let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("could not load image at \(path)\n".data(using: .utf8)!)
    exit(3)
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = ["en-US"]

let handler = VNImageRequestHandler(cgImage: cg, options: [:])
do {
    try handler.perform([request])
} catch {
    FileHandle.standardError.write("vision perform failed: \(error)\n".data(using: .utf8)!)
    exit(4)
}

guard let observations = request.results else {
    exit(0)
}

var lines: [String] = []
for obs in observations {
    if let top = obs.topCandidates(1).first {
        let s = top.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !s.isEmpty { lines.append(s) }
    }
}

print(lines.joined(separator: "\n"))
