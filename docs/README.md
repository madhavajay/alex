# Alex / Alexandria docs

Reference and design docs. Task tracking lives in the root [`TODO.md`](../TODO.md);
release notes in [`CHANGELOG.md`](../CHANGELOG.md).

| Doc | What it covers |
|---|---|
| [build-signed.md](build-signed.md) | Signed macOS build: `build-signed.sh`, Developer ID signing, notarization, and the GitHub secrets the release workflow expects. |
| [harnesses.md](harnesses.md) | Harness integration and session tracing — provider headers, dynamic header hooks, lifecycle hooks, and sub-agent lineage techniques. |
| [amp-wrap.md](amp-wrap.md) | `alex wrap amp` — self-contained reverse-wrap capture for the Sourcegraph Amp CLI (lives in `crates/alex-wrap`). |
| [credentials-plan.md](credentials-plan.md) | Design/roadmap for the credential vault (multi-account, budgets, model allow-lists, copy/reveal, audit). Open items are tracked in `TODO.md` §13. |
