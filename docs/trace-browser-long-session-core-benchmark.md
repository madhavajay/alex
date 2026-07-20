# Trace Browser long-session core benchmark

`TraceBrowserLongSessionBenchmarkTests` is a PR-safe Swift regression test for
the aggregate shape of a long-running agent session. Its checked-in JSON fixture
contains only counts and size parameters: 1,277 turns over 15 hours, 50-turn
pages, duplicate timestamps, message sizes, periodic tool activity, and one
synthetic search location. All text and identifiers are generated deterministically
at test time; no captured content, IDs, paths, headers, or hashes are present.

The test proves that AlexandriaBarCore can, within deliberately generous Mac CI
budgets:

- merge every page in stable timestamp/trace order without duplicate turns;
- refresh an existing tail page without increasing the turn count;
- filter a large transcript with bounded per-message search work;
- keep the classic transcript render window within its turn/character budgets;
- build a bounded attributed transcript with correct turn ranges;
- navigate the complete trace-id list; and
- use a changed `SessionSelection` id to reject a simulated stale page.

It does **not** instantiate `TraceBrowserModel`, `TranscriptChatPane`, SwiftUI,
an `NSWindow`, `URLSession`, the Alex daemon, or a LAR archive. It therefore does
not prove actual Task/network cancellation, initial time-to-interactive, lazy
view layout/painting, keyboard or pointer navigation, spinner stability, or
daemon-status stability. Those require the packaged-app automation benchmark in
the LAR plan. Passing this core test alone must not check the end-to-end macOS
Trace Browser task in `lar-format.md`.

Run it on macOS with:

```sh
cd macos
swift test --filter TraceBrowserLongSessionBenchmarkTests
```

The non-AppKit paging/filter/window/navigation cases can also run on other Swift
hosts. The attributed-string render case is compiled and run only when AppKit is
available, matching the existing macOS CI job.
