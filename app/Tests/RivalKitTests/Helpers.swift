import Foundation
import XCTest
@testable import RivalKit

let fixturesDir = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("Fixtures", isDirectory: true)

func fixtureNames() throws -> [String] {
    try FileManager.default.contentsOfDirectory(atPath: fixturesDir.path).filter { $0.hasSuffix(".json") }.sorted()
}

func loadFixture(_ name: String) throws -> Session {
    try Session.load(from: fixturesDir.appendingPathComponent(name))
}

/// A fresh directory under the system temp dir. Never ~/.rival.
func makeTempDir() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("rivalkit-test-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

/// The rival CLI tests' `sess` helper.
func sess(_ id: String, _ groupID: String, _ status: String, _ mode: String,
          _ cli: String, _ model: String, _ effort: String) -> Session {
    Session(id: id, groupID: groupID, cli: cli, mode: mode, model: model, effort: effort, status: status)
}

func solo(_ s: Session) -> RunItem { RunItem(id: "solo:" + s.id, sessions: [s]) }
func group(_ ss: [Session]) -> RunItem { RunItem(id: "group:g", sessions: ss) }

let solModel = "gpt-5.6-sol"
let claudeModel = "claude-opus-5-5"
