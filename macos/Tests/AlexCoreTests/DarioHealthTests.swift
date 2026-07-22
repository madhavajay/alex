import Foundation
import Testing
@testable import AlexCore

@Suite struct DarioHealthTests {
    @Test func readyGenerationIsGreen() throws {
        let evaluation = DarioHealth.evaluate(try status(phase: "ready", probeOK: true))
        #expect(evaluation.state == .ready)
        #expect(evaluation.tint == .green)
        #expect(evaluation.label == "ready")
    }

    @Test func warmingGenerationIsOrange() throws {
        let evaluation = DarioHealth.evaluate(try status(phase: "starting", probeOK: nil))
        #expect(evaluation.state == .warming)
        #expect(evaluation.tint == .orange)
        #expect(evaluation.label == "warming")
    }

    @Test func FailingProbeIsDownAndRed() throws {
        let evaluation = DarioHealth.evaluate(try status(phase: "ready", probeOK: false))
        #expect(evaluation.state == .down)
        #expect(evaluation.tint == .red)
        #expect(evaluation.label == "down")
    }

    @Test func noGenerationsIsDownAndRed() throws {
        let evaluation = DarioHealth.evaluate(try decode(
            #"{"active_generation_id":null,"generations":[]}"#))
        #expect(evaluation.state == .down)
        #expect(evaluation.tint == .red)
        #expect(evaluation.label == "down")
    }

    @Test func deadGenerationIsDownAndRed() throws {
        let evaluation = DarioHealth.evaluate(try status(phase: "dead", probeOK: nil))
        #expect(evaluation.state == .down)
        #expect(evaluation.tint == .red)
        #expect(evaluation.label == "down")
    }

    @Test func nilStatusWhileEnabledIsDownAndRed() {
        let status: DarioStatus? = nil
        let evaluation = DarioHealth.evaluate(status)
        #expect(evaluation.state == .down)
        #expect(evaluation.tint == .red)
        #expect(evaluation.label == "down")
    }

    private func status(phase: String, probeOK: Bool?) throws -> DarioStatus {
        let probe = probeOK.map { #", "last_probe":{"ok":\#($0)}"# } ?? ""
        return try decode(
            #"{"active_generation_id":"gen-1","generations":[{"id":"gen-1","version":"1.0.0","phase":"\#(phase)"\#(probe)}]}"#)
    }

    private func decode(_ json: String) throws -> DarioStatus {
        try JSONDecoder().decode(DarioStatus.self, from: Data(json.utf8))
    }
}
