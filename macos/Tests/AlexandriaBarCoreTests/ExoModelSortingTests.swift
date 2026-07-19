import Testing
@testable import AlexandriaBarCore

@Suite struct ExoModelSortingTests {
    @Test func sortsRunningThenEnabledThenNameAndPreservesTies() {
        let models = [
            model(id: "disabled-zulu", name: "Zulu"),
            model(id: "enabled-zulu", name: "zulu", enabled: true),
            model(id: "running-beta", name: "beta", running: true),
            model(id: "running-enabled-zulu", name: "Zulu", enabled: true, running: true),
            model(id: "enabled-alpha", name: "Alpha", enabled: true),
            model(id: "running-alpha-first", name: "alpha", running: true),
            model(id: "running-alpha-second", name: "ALPHA", running: true),
            model(id: "disabled-alpha", name: "alpha"),
        ]

        #expect(models.sortedForDisplay().map(\.id) == [
            "running-enabled-zulu",
            "running-alpha-first",
            "running-alpha-second",
            "running-beta",
            "enabled-alpha",
            "enabled-zulu",
            "disabled-alpha",
            "disabled-zulu",
        ])
    }

    private func model(
        id: String,
        name: String,
        enabled: Bool = false,
        running: Bool? = nil
    ) -> ExoModel {
        ExoModel(
            id: id,
            name: name,
            family: nil,
            quantization: nil,
            contextLength: nil,
            enabled: enabled,
            running: running
        )
    }
}
