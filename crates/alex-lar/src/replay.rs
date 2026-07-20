use std::io::Write;
use std::time::Duration;

use crate::{Error, ParsedFrame, Result, StreamFrameKind, StreamIndex, StreamParser, StreamRead};

/// Which independently replayable view of a captured stream to emit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamReplaySource {
    /// The exact byte ranges returned by the upstream HTTP client's reads.
    ObservedReads,
    /// Parsed SSE/NDJSON ranges. Gaps and unparsed framing are intentionally
    /// omitted; use `ObservedReads` for byte-exact transport replay.
    ParsedFrames,
}

/// Timing policy for stream replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamReplayTiming {
    /// Emit every range without sleeping.
    Instant,
    /// Preserve the observed delays from first byte.
    Original,
    /// Replay at `speed_numerator / speed_denominator` times real time.
    /// For example, `{ 2, 1 }` is 2x and `{ 1, 4 }` is 0.25x.
    Scaled {
        speed_numerator: u32,
        speed_denominator: u32,
    },
}

/// One range in a replay schedule. Bytes remain owned once by `StreamReplay`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamReplayEvent {
    pub byte_offset: u64,
    pub byte_length: u64,
    pub observed_delta_ns: u64,
    pub wait_before_ns: u64,
    pub parser: Option<StreamParser>,
    pub frame_kind: Option<StreamFrameKind>,
}

/// A verified raw body plus a zero-copy schedule over its captured ranges.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamReplay {
    body: Vec<u8>,
    events: Vec<StreamReplayEvent>,
}

impl StreamReplay {
    pub(crate) fn from_index(
        index: &StreamIndex,
        body: Vec<u8>,
        source: StreamReplaySource,
        timing: StreamReplayTiming,
    ) -> Result<Self> {
        validate_timing(timing)?;
        let body_length = body.len() as u64;
        let events = match source {
            StreamReplaySource::ObservedReads => schedule_reads(&index.reads, timing)?,
            StreamReplaySource::ParsedFrames => schedule_frames(&index.frames, timing)?,
        };
        for event in &events {
            let end = event
                .byte_offset
                .checked_add(event.byte_length)
                .ok_or(Error::Invalid("stream replay range overflow"))?;
            if end > body_length {
                return Err(Error::Invalid("stream replay range exceeds body"));
            }
        }
        Ok(Self { body, events })
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn events(&self) -> &[StreamReplayEvent] {
        &self.events
    }

    pub fn event_bytes(&self, event: &StreamReplayEvent) -> Result<&[u8]> {
        let start = usize::try_from(event.byte_offset)
            .map_err(|_| Error::Invalid("stream replay range overflow"))?;
        let end_u64 = event
            .byte_offset
            .checked_add(event.byte_length)
            .ok_or(Error::Invalid("stream replay range overflow"))?;
        let end =
            usize::try_from(end_u64).map_err(|_| Error::Invalid("stream replay range overflow"))?;
        self.body
            .get(start..end)
            .ok_or(Error::Invalid("stream replay range exceeds body"))
    }

    /// Play the schedule using an injected sleeper. This is the deterministic
    /// primitive used by UIs, CLIs, and tests; output is flushed at each event
    /// so the original read boundaries remain externally visible.
    pub fn play_to<W, S>(&self, mut output: W, mut sleep: S) -> Result<u64>
    where
        W: Write,
        S: FnMut(Duration),
    {
        let mut written = 0u64;
        for event in &self.events {
            if event.wait_before_ns != 0 {
                sleep(Duration::from_nanos(event.wait_before_ns));
            }
            let bytes = self.event_bytes(event)?;
            output.write_all(bytes)?;
            output.flush()?;
            written = written
                .checked_add(bytes.len() as u64)
                .ok_or(Error::Invalid("stream replay output length overflow"))?;
        }
        Ok(written)
    }

    /// Blocking real-time playback for command-line and test-fixture use.
    /// Async callers should use `play_to` on a blocking worker.
    pub fn play_to_realtime<W: Write>(&self, output: W) -> Result<u64> {
        self.play_to(output, std::thread::sleep)
    }
}

fn validate_timing(timing: StreamReplayTiming) -> Result<()> {
    if let StreamReplayTiming::Scaled {
        speed_numerator,
        speed_denominator,
    } = timing
    {
        if speed_numerator == 0 || speed_denominator == 0 {
            return Err(Error::Invalid("stream replay speed must be non-zero"));
        }
    }
    Ok(())
}

fn scale_delay(delay_ns: u64, timing: StreamReplayTiming) -> Result<u64> {
    match timing {
        StreamReplayTiming::Instant => Ok(0),
        StreamReplayTiming::Original => Ok(delay_ns),
        StreamReplayTiming::Scaled {
            speed_numerator,
            speed_denominator,
        } => {
            let value = u128::from(delay_ns)
                .checked_mul(u128::from(speed_denominator))
                .ok_or(Error::Invalid("stream replay delay overflow"))?
                / u128::from(speed_numerator);
            u64::try_from(value).map_err(|_| Error::Invalid("stream replay delay overflow"))
        }
    }
}

fn wait_before(current_delta: u64, previous_delta: u64, timing: StreamReplayTiming) -> Result<u64> {
    let observed = current_delta
        .checked_sub(previous_delta)
        .ok_or(Error::Invalid("stream replay timing is not monotonic"))?;
    scale_delay(observed, timing)
}

fn schedule_reads(
    reads: &[StreamRead],
    timing: StreamReplayTiming,
) -> Result<Vec<StreamReplayEvent>> {
    let mut previous_delta = 0;
    reads
        .iter()
        .map(|read| {
            let event = StreamReplayEvent {
                byte_offset: read.byte_offset,
                byte_length: read.byte_length,
                observed_delta_ns: read.delta_from_first_byte_ns,
                wait_before_ns: wait_before(read.delta_from_first_byte_ns, previous_delta, timing)?,
                parser: None,
                frame_kind: None,
            };
            previous_delta = read.delta_from_first_byte_ns;
            Ok(event)
        })
        .collect()
}

fn schedule_frames(
    frames: &[ParsedFrame],
    timing: StreamReplayTiming,
) -> Result<Vec<StreamReplayEvent>> {
    let mut previous_delta = 0;
    frames
        .iter()
        .map(|frame| {
            let event = StreamReplayEvent {
                byte_offset: frame.byte_offset,
                byte_length: frame.byte_length,
                observed_delta_ns: frame.delta_from_first_byte_ns,
                wait_before_ns: wait_before(
                    frame.delta_from_first_byte_ns,
                    previous_delta,
                    timing,
                )?,
                parser: Some(frame.parser),
                frame_kind: Some(frame.frame_kind),
            };
            previous_delta = frame.delta_from_first_byte_ns;
            Ok(event)
        })
        .collect()
}
