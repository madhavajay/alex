# Trace Browser packaged macOS benchmark

This benchmark is the end-to-end performance and stability gate for a real
`Alex.app` Trace Browser window. It uses only deterministic, aggregate-shaped
generated data: a 1,277-turn session spanning 15 hours and a distinct three-turn
session. It never reads a captured trace corpus or the user's normal Alex home.

## What runs

`macos/Scripts/run_trace_browser_benchmark.sh`:

1. Builds the release `alex` CLI/daemon and packages `macos/dist/Alex.app`.
2. Generates the two synthetic sessions inside a `mktemp` `ALEXANDRIA_HOME`.
3. Imports every legacy body into LAR, verifies the live LAR catalog and every
   published artifact pointer, then removes the generated gzip body directory
   so browser reads must come from LAR.
4. Starts an actual daemon with `ALEXANDRIA_LAR_BODY_STORE=lar-with-fallback`.
5. Places a loopback reverse proxy between the packaged app and daemon. The
   proxy forwards every production endpoint and only delays the first two
   older-page responses (0.25 s and 1.5 s) to make loading and stale-result
   suppression observable.
6. Launches `Alex.app/Contents/MacOS/AlexandriaBar` directly in an explicit
   benchmark mode. Normal status-item startup is unchanged when that mode is
   absent.

The app opens a real `NSWindow` hosting `TraceBrowserView`. Internal SwiftUI
preference markers prove that the session and transcript panes committed, and
record every activation of the session/transcript/page spinners and daemon-down
banner. No Accessibility or Screen Recording permission is needed.

## Run locally

From the repository root on a Mac with Xcode/Swift, Rust, Python 3, and `curl`:

```bash
macos/Scripts/run_trace_browser_benchmark.sh \
  --result /tmp/trace-browser-benchmark.json \
  --artifacts-dir /tmp/trace-browser-benchmark-artifacts
```

Use `--skip-build` to reuse `target/release/alex` and
`macos/dist/Alex.app`. Use `--keep-work` only when debugging; it retains the
otherwise-deleted generated archive and isolated config.

The manual GitHub Actions workflow is **Trace Browser packaged benchmark**
(`.github/workflows/trace-browser-benchmark.yml`). It uploads the JSON result,
daemon/app/proxy logs, the import report, `lar-verification.json`, and the
aggregate-only corpus manifest.

## Required phases and output

The result schema is `alex-trace-browser-packaged-benchmark-v1`. A passing run
has `passed: true` and covers:

- targeted initial tail load: exactly 50 of 1,277 turns;
- one older page: exactly 100 of 1,277 turns;
- navigation to the three-turn session while a delayed older page is in flight,
  followed by a wait beyond the delay to prove stale-result suppression;
- navigation back, jump to latest, and adjacent inspector trace navigation;
- ten one-second poll intervals with no reactivation of session, transcript, or
  page loading and no daemon-down state;
- a visible, key, attached, non-empty window and committed pane markers;
- per-phase durations, at least 100 main-actor heartbeat samples, and the
  maximum 10 ms heartbeat gap (250 ms budget).

The delayed-page phase proves that a response from the old transcript generation
cannot overwrite the newly selected session. It does not claim the underlying
URLSession transport was cancelled. URLSession's `.cancelled` error is normalized
to `CancellationError` so genuine cancellation cannot flash a daemon outage.

## Validation boundary

The generated corpus, proxy, runner shell, and JSON checks can be statically
validated on Linux. AppKit, SwiftUI view commits, release packaging, WindowServer
visibility, and timing budgets can only pass on macOS. The LAR plan's packaged
end-to-end checkbox must remain unchecked until this workflow or the local command
actually passes on a Mac.

The [long-session core benchmark](trace-browser-long-session-core-benchmark.md)
remains useful for fast paging/filter/render regression tests, but it is not a
substitute for this packaged-app gate.
