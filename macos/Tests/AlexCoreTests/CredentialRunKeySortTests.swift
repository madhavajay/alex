import Foundation
import Testing
@testable import AlexCore

@Suite struct CredentialRunKeySortTests {
    private func keys() throws -> [CredentialRunKey] {
        let json = #"""
        [
          {"id":"rk-zulu","key_fingerprint":"z","kind":"harness","label":"Zulu","run_id":null,"tags":{},"created_ms":300,"expires_ms":900,"last_used_ms":700,"use_count":2,"revoked":false},
          {"id":"rk-alpha","key_fingerprint":"a","kind":"run","label":"alpha","run_id":null,"tags":{},"created_ms":100,"expires_ms":null,"last_used_ms":500,"use_count":9,"revoked":false},
          {"id":"rk-kind","key_fingerprint":"k","kind":"wrap","label":null,"run_id":null,"tags":{},"created_ms":200,"expires_ms":600,"last_used_ms":null,"use_count":4,"revoked":true}
        ]
        """#
        return try JSONDecoder().decode([CredentialRunKey].self, from: Data(json.utf8))
    }

    @Test func sortsEveryCredentialColumnInBothDirections() throws {
        let values = try keys()
        #expect(values.sorted(by: .label, direction: .ascending).map(\.id)
            == ["rk-alpha", "rk-kind", "rk-zulu"])
        #expect(values.sorted(by: .label, direction: .descending).map(\.id)
            == ["rk-zulu", "rk-kind", "rk-alpha"])
        #expect(values.sorted(by: .created, direction: .ascending).map(\.id)
            == ["rk-alpha", "rk-kind", "rk-zulu"])
        #expect(values.sorted(by: .created, direction: .descending).map(\.id)
            == ["rk-zulu", "rk-kind", "rk-alpha"])
        #expect(values.sorted(by: .uses, direction: .ascending).map(\.id)
            == ["rk-zulu", "rk-kind", "rk-alpha"])
        #expect(values.sorted(by: .uses, direction: .descending).map(\.id)
            == ["rk-alpha", "rk-kind", "rk-zulu"])
    }

    @Test func optionalDatesKeepMissingValuesLast() throws {
        let values = try keys()
        #expect(values.sorted(by: .expires, direction: .ascending).map(\.id)
            == ["rk-kind", "rk-zulu", "rk-alpha"])
        #expect(values.sorted(by: .expires, direction: .descending).map(\.id)
            == ["rk-zulu", "rk-kind", "rk-alpha"])
        #expect(values.sorted(by: .lastUsed, direction: .ascending).map(\.id)
            == ["rk-alpha", "rk-zulu", "rk-kind"])
        #expect(values.sorted(by: .lastUsed, direction: .descending).map(\.id)
            == ["rk-zulu", "rk-alpha", "rk-kind"])
    }
}
