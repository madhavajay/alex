# Trace Browser: Crash-on-Scroll Fix + Performance Test Plan

Goal: the macOS Trace Browser must NEVER crash or beach-ball on long traces.
All heavy work moves off the UI's critical path (Rust background threads +
Swift off-main), cancellation is aggressive (click off a trace → every fetch
for it is abandoned, client AND daemon side), and a large-fake-trace corpus
proves it under test.

## Diagnosis (from code exploration, file:line verified)

The scroll crash/hang is not one bug — it's four compounding costs:

1. **Daemon rebuilds the whole transcript every 500 ms.** The macOS app polls
   `GET /traces/sessions/{id}/transcript` (limit 500) on a 500 ms timer
   (`TraceBrowserWindow.swift:662-667`, `:948`). The handler
   (`alex-proxy/src/lib.rs:6453`) synchronously — no `spawn_blocking`, no
   awaits — reads, gunzips, and JSON-parses EVERY turn's full request AND
   response body from disk (up to 64 MB each, `transcript_turn` `:6084-6161`)
   before truncating for display. All of it behind the store's single global
   `Mutex<Connection>` (`alex-store/src/lib.rs:881-882`).
2. **The daemon cannot cancel abandoned work.** Because the heavy section has
   no await points, hyper/axum cannot drop the future on client disconnect.
   Each cancelled 500 ms poll still runs to completion server-side, stacking
   full-transcript rebuilds on one mutex while the client has moved on.
3. **Classic text pane forces layout on the main thread per scroll frame.**
   `TranscriptTextPane.drawBackground → drawBubbles` calls
   `layoutManager.ensureLayout` + fragment enumeration per bubble group per
   draw pass over one giant NSTextView document (`TranscriptTextPane.swift:24-116`).
   Most likely native crash/watchdog candidate.
4. **Unbounded memory on the client.** `traceBody`/`toolBody` load whole
   bodies into Strings with no client-side cap (`AlexClient.swift:942-983`);
   `toolBodyCache` is an unbounded dict (`TraceBrowserWindow.swift:262`);
   the inspector eagerly pretty-prints UNCAPPED bodies on the MainActor
   (`BodyPretty.display(raw, cap: .max)` in view bodies,
   `TraceInspectorPane.swift:191, 759, 771, 804`).

Good news: lighter endpoints already exist and are unused by the app —
body-free cursor-paginated `/transcript/page` (20/page, `lib.rs:6505`) and
single-turn `/traces/{id}/turn` (`:6594`). The web UI uses them; macOS doesn't.

## Fix plan

### Rust daemon (the root cause)

R1. **Move trace read work off the async threads**: wrap the heavy sections of
    `traces_session_transcript`, `trace_body`, `tool_body`, `trace_reply_md`,
    and `trace_turn` in `spawn_blocking` so worker threads never stall and
    disconnects can drop the awaiting future at the spawn boundary.
R2. **Cooperative cancellation inside the transcript build**: pass an
    `Arc<AtomicBool>`/token flipped when the request is dropped (guard object
    on the future) into `build_session_transcript`; check it between turns and
    bail early. An abandoned poll then costs at most one turn's read, not 500.
R3. **Incremental transcript cache**: the 500 ms poll re-reads bodies for turns
    that cannot have changed. Cache built transcript turns keyed by
    (trace_id, completed_ms) in a bounded LRU inside the daemon; only new/
    changed turns get body reads. Poll cost becomes O(new turns), not O(all).
R4. **Reduce lock scope**: body file reads (disk + gunzip) currently hold no
    DB lock, but row fetches serialize everything. Ensure the connection lock
    is released before file I/O in the touched handlers (verify; fix where not).
R5. Do NOT change endpoint semantics — the web UI depends on them.

### macOS app

S1. **Switch the transcript poll to the paged/body-free flow**: poll
    `/transcript/page` for metadata (cheap), fetch full turn content only for
    turns that are (near-)visible via `/traces/{id}/turn`, with a small
    prefetch window. The 500 ms full-transcript poll disappears entirely.
S2. **Aggressive cancellation on click-off**: selection change or window close
    cancels every in-flight task for the previous session (largely exists:
    `resetTurns()`/`stop()`) — extend to the new per-turn fetches; key each
    fetch by session id and drop results for stale keys (pattern exists at
    `:838`). Store handles for the currently fire-and-forget Tasks
    (`revealSessionBodies`, reply/delete flows) so window close cancels them.
S3. **Classic pane scroll cost**: cache bubble-group rects after layout instead
    of `ensureLayout` per draw; invalidate on storage edits. If that's
    insufficient under the perf test, cap the classic window harder (it's
    opt-in; default chat pane is LazyVStack and fine).
S4. **Bound client memory**: cap `traceBody`/`toolBody` responses client-side
    (daemon already 413s over 64 MB; add a sane display cap ~2 MB with a
    "load full body" escape hatch); make `toolBodyCache` an LRU like
    `TraceBodyCache`; move the eager `BodyPretty.display(_, cap: .max)` calls
    off the MainActor and cap them.
S5. **Rendered-artifact cache**: cache expensive per-turn chat-pane content
    after `TurnTextCap`, pretty-printed/highlighted JSON output, and parsed SSE
    frame pages. Key entries by `(trace_id, completed_ms,
    render-config-discriminator)` so unchanged turns hit and changed turns
    miss. Bound the LRU by count and approximate byte cost, clear it on memory
    pressure and window close, and sweep entries untouched for about five
    minutes with an injectable clock. Do not cache the classic pane's
    monolithic attributed string; S3's rectangle cache remains its fast path.

### Test data + proof (build first, so the fix is measured, not vibed)

T1. **Corpus generator**: extend the existing `alex-lar-scale` machinery
    (`generate_corpus`, 55k-trace profile) with a `trace-browser` profile:
    one session with 500 long turns (multi-MB streamed bodies, tool calls,
    SSE), plus 50 medium sessions, seeded directly via `Store::insert_trace`
    + `write_body`. Ship as `cargo run -p alex-lar-scale -- browser-profile
    --root <tmp>` or a test.sh helper so anyone can reproduce.
T2. **Daemon perf tests** (cargo, no UI): against the T1 corpus assert
    (a) transcript endpoint p95 under a budget with the cache warm,
    (b) an aborted request stops consuming the store lock (measure lock wait
    of a concurrent request while a poll is abandoned mid-build),
    (c) memory ceiling during 100 sequential polls (no growth ⇒ cache bounded).
T3. **Swift perf/regression tests**: extend the `TranscriptFilterTests`
    pattern — feed the model 500 synthetic long turns; assert filter, window
    rebuild, and (new) rect-cache classic-pane layout stay under wall-clock
    budgets; assert cancellation: select session B while A's turn fetches are
    in flight → A's tasks are cancelled (spy client records cancellations).
    Cover the S5 cache with an unchanged 500-turn second render that is at
    least 10× faster or under a tight budget, a changed-`completed_ms` miss,
    memory-pressure clearing, and a fake-clock idle sweep without real sleeps.
T4. **UI smoke under corpus** (manual + optional XCUITest later): script that
    boots daemon with the T1 corpus + opens the app for hand-testing scroll.

## Invariants

- The UI never crashes on any trace size the daemon can store (64 MB bodies).
- Scrolling never blocks the main thread on network, disk, parse, or layout.
- Click-off cancels: no fetch for a deselected trace continues client-side,
  and the daemon abandons its work at the next turn boundary.
- Bounded memory: every cache LRU-bounded; no uncapped String materialization
  on the MainActor.
- The web UI's transcript endpoints keep identical semantics.

## Delivery

Worktree `wt-trace-browser`, branch `fix/trace-browser-scroll-crash`, off
merged main. Order: T1 corpus → T2 daemon tests (red) → R1–R4 (green) →
S1–S4 with T3 (green) → S5 with extended T3 (green) → PR with before/after
numbers from T2/T3 and S5 cache hit/miss timings.
