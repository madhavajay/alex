import Testing
@testable import AlexCore

@Suite struct SessionForkCommandTests {
    @Test func uuidStaysReadable() {
        #expect(
            SessionForkCommand.command(
                sessionId: "5d56cba0-b43b-464a-99fb-5bbca2bcc46d")
                == "alex resume 5d56cba0-b43b-464a-99fb-5bbca2bcc46d")
    }

    @Test func commonSessionIdPunctuationDoesNotNeedQuoting() {
        #expect(
            SessionForkCommand.command(sessionId: "auto/session_1.2:run-3")
                == "alex resume auto/session_1.2:run-3")
    }

    @Test func shellMetacharactersAreQuoted() {
        #expect(
            SessionForkCommand.command(sessionId: "session id; echo unsafe")
                == "alex resume 'session id; echo unsafe'")
    }

    @Test func apostrophesAreEscapedInsideQuotedIds() {
        #expect(
            SessionForkCommand.command(sessionId: "team's session")
                == "alex resume 'team'\\''s session'")
    }

    @Test func emptyIdIsStillOneShellArgument() {
        #expect(SessionForkCommand.command(sessionId: "") == "alex resume ''")
    }
}
