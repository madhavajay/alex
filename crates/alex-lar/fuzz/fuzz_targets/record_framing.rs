#![no_main]

mod common;

use alex_lar::{read_file_header, ArchiveReader, FrameRead, FrameReader};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let data = common::bounded(data);
    let limits = common::limits();
    let _ = read_file_header(&mut Cursor::new(data), &limits);

    let mut cursor = Cursor::new(data);
    for _ in 0..4_096 {
        match FrameReader::new(&mut cursor, &limits).read_next() {
            Ok((FrameRead::Frame, Some(_))) => {}
            _ => break,
        }
    }
    let _ = ArchiveReader::open(Cursor::new(data), limits);
});
