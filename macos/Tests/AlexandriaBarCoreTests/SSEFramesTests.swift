import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct SSEFramesTests {
    @Test func isSSEDetectsDataLines() {
        #expect(SSEFrames.isSSE("event: message_start\ndata: {\"type\":\"start\"}\n\n"))
        #expect(SSEFrames.isSSE("data: {\"a\":1}\n\ndata: {\"a\":2}\n\n"))
        #expect(!SSEFrames.isSSE(#"{"model": "claude-opus-4-8"}"#))
        #expect(!SSEFrames.isSSE("plain text response body"))
        #expect(!SSEFrames.isSSE(""))
    }

    @Test func parsesEventAndDataFrames() {
        let body = """
            event: message_start
            data: {"type":"message_start"}

            event: content_block_delta
            data: {"delta":"hi"}

            """
        let (frames, truncated) = SSEFrames.parse(body)
        #expect(!truncated)
        #expect(frames.count == 2)
        #expect(frames[0].event == "message_start")
        #expect(frames[0].data == #"{"type":"message_start"}"#)
        #expect(frames[1].event == "content_block_delta")
        #expect(frames[1].data == #"{"delta":"hi"}"#)
    }

    @Test func multiLineDataJoinsWithNewline() {
        let body = """
            data: line one
            data: line two

            """
        let (frames, _) = SSEFrames.parse(body)
        #expect(frames.count == 1)
        #expect(frames[0].data == "line one\nline two")
        #expect(frames[0].event == nil)
    }

    @Test func commentLinesAndUnknownFieldsAreIgnored() {
        let body = """
            : keep-alive
            id: 42
            retry: 3000
            data: hello

            """
        let (frames, _) = SSEFrames.parse(body)
        #expect(frames.count == 1)
        #expect(frames[0].data == "hello")
    }

    @Test func stopsAtMaxFramesAndReportsTruncation() {
        let body = (0..<10).map { "data: frame\($0)\n" }.joined(separator: "\n")
        let (frames, truncated) = SSEFrames.parse(body, maxFrames: 3)
        #expect(frames.count == 3)
        #expect(truncated)
    }

    @Test func handlesFrameWithoutTrailingBlankLine() {
        let body = "data: only frame, no trailing newline"
        let (frames, truncated) = SSEFrames.parse(body)
        #expect(!truncated)
        #expect(frames.count == 1)
        #expect(frames[0].data == "only frame, no trailing newline")
    }
}
