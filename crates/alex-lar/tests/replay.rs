use std::io::Cursor;
use std::time::Duration;

use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits, ParsedFrame, StreamFrameKind,
    StreamIndex, StreamParser, StreamRead, StreamReplaySource, StreamReplayTiming,
};

fn archive() -> (Vec<u8>, alex_lar::StreamIndexId) {
    let limits = Limits {
        max_chunk_uncompressed: 128,
        ..Limits::default()
    };
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        FileHeader::standalone([0x72; 16], 1, b"replay-test".to_vec()),
        ChunkerConfig {
            min_size: 32,
            target_size: 64,
            max_size: 128,
        },
        limits,
    )
    .unwrap();
    let body = b"data: a\n\nignored\ndata: b\n\n";
    let body_id = writer.append_body(body).unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            body_id,
            vec![
                StreamRead {
                    byte_offset: 0,
                    byte_length: 9,
                    delta_from_first_byte_ns: 0,
                },
                StreamRead {
                    byte_offset: 9,
                    byte_length: 8,
                    delta_from_first_byte_ns: 2_000_000,
                },
                StreamRead {
                    byte_offset: 17,
                    byte_length: 9,
                    delta_from_first_byte_ns: 8_000_000,
                },
            ],
            vec![
                ParsedFrame {
                    byte_offset: 0,
                    byte_length: 9,
                    delta_from_first_byte_ns: 0,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
                ParsedFrame {
                    byte_offset: 17,
                    byte_length: 9,
                    delta_from_first_byte_ns: 8_000_000,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
            ],
        ))
        .unwrap();
    writer.seal().unwrap();
    (writer.into_inner().unwrap().into_inner(), stream_id)
}

#[test]
fn observed_reads_replay_exact_bytes_with_original_or_instant_timing() {
    let (bytes, stream_id) = archive();
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    let replay = reader
        .read_stream_replay(
            &stream_id,
            StreamReplaySource::ObservedReads,
            StreamReplayTiming::Original,
        )
        .unwrap();
    assert_eq!(
        replay
            .events()
            .iter()
            .map(|event| event.wait_before_ns)
            .collect::<Vec<_>>(),
        vec![0, 2_000_000, 6_000_000]
    );
    let mut output = Vec::new();
    let mut sleeps = Vec::new();
    assert_eq!(
        replay
            .play_to(&mut output, |duration| sleeps.push(duration))
            .unwrap(),
        26
    );
    assert_eq!(output, b"data: a\n\nignored\ndata: b\n\n");
    assert_eq!(
        sleeps,
        vec![Duration::from_millis(2), Duration::from_millis(6)]
    );

    let instant = reader
        .read_stream_replay(
            &stream_id,
            StreamReplaySource::ObservedReads,
            StreamReplayTiming::Instant,
        )
        .unwrap();
    assert!(instant
        .events()
        .iter()
        .all(|event| event.wait_before_ns == 0));
}

#[test]
fn scaled_and_parsed_replay_use_independent_ranges() {
    let (bytes, stream_id) = archive();
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    let replay = reader
        .read_stream_replay(
            &stream_id,
            StreamReplaySource::ParsedFrames,
            StreamReplayTiming::Scaled {
                speed_numerator: 4,
                speed_denominator: 1,
            },
        )
        .unwrap();
    assert_eq!(replay.events().len(), 2);
    assert_eq!(replay.events()[1].wait_before_ns, 2_000_000);
    assert_eq!(replay.events()[0].parser, Some(StreamParser::Sse));
    let mut output = Vec::new();
    replay.play_to(&mut output, |_| {}).unwrap();
    assert_eq!(output, b"data: a\n\ndata: b\n\n");
}

#[test]
fn invalid_scaled_speed_is_rejected() {
    let (bytes, stream_id) = archive();
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    let error = reader
        .read_stream_replay(
            &stream_id,
            StreamReplaySource::ObservedReads,
            StreamReplayTiming::Scaled {
                speed_numerator: 0,
                speed_denominator: 1,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("speed must be non-zero"));
}

#[test]
fn checked_body_ranges_cross_chunks_without_reconstructing_the_whole_body() {
    let (bytes, stream_id) = archive();
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    let manifest_id = reader
        .stream_index(&stream_id)
        .unwrap()
        .raw_body_manifest_id;
    let ranges = reader
        .read_body_ranges(&manifest_id, &[(0, 9), (17, 9), (8, 10)])
        .unwrap();
    assert_eq!(ranges[0], b"data: a\n\n");
    assert_eq!(ranges[1], b"data: b\n\n");
    assert_eq!(ranges[2], b"\nignored\nd");
    assert_eq!(
        reader.read_body_range(&manifest_id, 9, 8).unwrap(),
        b"ignored\n"
    );

    let error = reader.read_body_range(&manifest_id, 25, 2).unwrap_err();
    assert!(error.to_string().contains("range exceeds manifest"));
}
