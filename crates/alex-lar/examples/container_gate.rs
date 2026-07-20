//! Compare the same content-addressed body/manifest workload in native LAR and
//! a standards-valid MCAP profile.
//!
//! This is a design-gate executable, not an MCAP backend. The MCAP profile
//! stores each zstd-compressed unique chunk once as an indexed attachment and
//! each body manifest as an indexed message. That is the closest portable MCAP
//! mapping which preserves LAR's one-copy body invariant.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;
use std::time::{Duration, Instant};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits, ManifestId, StreamingChunker,
};
use enumset::enum_set;
use mcap::read::{attachment, ChunkFlattener, Options};
use mcap::records::{MessageHeader, Record};
use mcap::{Attachment, Compression, Summary, WriteOptions, Writer};
use serde_json::json;

const PROFILE: &str = "org.alexandria.lar-mcap-profile-v0";
const CHUNK_MEDIA_TYPE: &str = "application/vnd.alexandria.lar-chunk+zstd";
const MANIFEST_TOPIC: &str = "alex.body.manifest";

#[derive(Clone, Debug)]
struct ChunkRef {
    digest: [u8; 32],
    logical_offset: u64,
    length: u64,
}

#[derive(Clone, Debug)]
struct Manifest {
    whole_hash: [u8; 32],
    total_length: u64,
    chunks: Vec<ChunkRef>,
}

#[derive(Debug)]
struct ProfileData {
    manifests: Vec<Manifest>,
    chunks: BTreeMap<[u8; 32], Vec<u8>>,
}

fn synthetic_bodies(turns: usize) -> Vec<Vec<u8>> {
    let mut transcript = String::from(
        r#"{"model":"synthetic-code","messages":[{"role":"system","content":"deterministic agent"}"#,
    );
    let tool_blob = (0..512)
        .map(|line| {
            format!(
                "{line:04} module-{}/file-{}.rs {}\n",
                line % 37,
                line % 211,
                blake3::hash(format!("tool-line-{line}").as_bytes()).to_hex()
            )
        })
        .collect::<String>();
    let mut bodies = Vec::with_capacity(turns);
    for turn in 0..turns {
        transcript.push_str(&format!(
            r#",{{"role":"user","content":"turn {turn}"}},{{"role":"assistant","content":"answer {turn}"}}"#
        ));
        if turn % 7 == 6 {
            transcript.push_str(&format!(
                r#",{{"role":"tool","tool_call_id":"call-{turn}","content":{}}}"#,
                serde_json::to_string(&tool_blob).unwrap()
            ));
        }
        let mut request = transcript.as_bytes().to_vec();
        request.extend_from_slice(br#"],"stream":true}"#);
        bodies.push(request);
    }
    bodies
}

fn profile_data(bodies: &[Vec<u8>]) -> Result<ProfileData, String> {
    let mut chunks = BTreeMap::<[u8; 32], Vec<u8>>::new();
    let mut manifests = Vec::with_capacity(bodies.len());
    for body in bodies {
        let mut chunker =
            StreamingChunker::new(ChunkerConfig::default()).map_err(|error| error.to_string())?;
        let mut refs = Vec::new();
        let mut logical_offset = 0_u64;
        let mut append = |bytes: &[u8]| -> alex_lar::Result<()> {
            let digest = *blake3::hash(bytes).as_bytes();
            if let Some(existing) = chunks.get(&digest) {
                if existing != bytes {
                    return Err(alex_lar::Error::Invalid("BLAKE3 chunk collision"));
                }
            } else {
                chunks.insert(digest, bytes.to_vec());
            }
            refs.push(ChunkRef {
                digest,
                logical_offset,
                length: bytes.len() as u64,
            });
            logical_offset += bytes.len() as u64;
            Ok(())
        };
        chunker
            .push(body, &mut append)
            .map_err(|error| error.to_string())?;
        chunker
            .finish(&mut append)
            .map_err(|error| error.to_string())?;
        manifests.push(Manifest {
            whole_hash: *blake3::hash(body).as_bytes(),
            total_length: body.len() as u64,
            chunks: refs,
        });
    }
    Ok(ProfileData { manifests, chunks })
}

fn encode_manifest(manifest: &Manifest) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(44 + manifest.chunks.len() * 48);
    bytes.extend_from_slice(&manifest.whole_hash);
    bytes.extend_from_slice(&manifest.total_length.to_le_bytes());
    bytes.extend_from_slice(&(manifest.chunks.len() as u32).to_le_bytes());
    for reference in &manifest.chunks {
        bytes.extend_from_slice(&reference.digest);
        bytes.extend_from_slice(&reference.logical_offset.to_le_bytes());
        bytes.extend_from_slice(&reference.length.to_le_bytes());
    }
    bytes
}

fn decode_manifest(mut bytes: &[u8]) -> Result<Manifest, String> {
    fn take<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], String> {
        let value = input
            .get(..N)
            .ok_or_else(|| "truncated MCAP profile manifest".to_string())?;
        *input = &input[N..];
        Ok(value.try_into().unwrap())
    }
    let whole_hash = take::<32>(&mut bytes)?;
    let total_length = u64::from_le_bytes(take::<8>(&mut bytes)?);
    let count = u32::from_le_bytes(take::<4>(&mut bytes)?) as usize;
    if count > 1_000_000 {
        return Err("MCAP profile manifest chunk count exceeds limit".into());
    }
    let expected = count
        .checked_mul(48)
        .ok_or("MCAP profile manifest length overflow")?;
    if bytes.len() != expected {
        return Err("MCAP profile manifest length mismatch".into());
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        chunks.push(ChunkRef {
            digest: take::<32>(&mut bytes)?,
            logical_offset: u64::from_le_bytes(take::<8>(&mut bytes)?),
            length: u64::from_le_bytes(take::<8>(&mut bytes)?),
        });
    }
    Ok(Manifest {
        whole_hash,
        total_length,
        chunks,
    })
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn build_lar(bodies: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<ManifestId>), String> {
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        FileHeader::standalone([0x4c; 16], 1, b"container-gate".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .map_err(|error| error.to_string())?;
    writer.enable_metadata_pages();
    let mut ids = Vec::with_capacity(bodies.len());
    for body in bodies {
        ids.push(
            writer
                .append_body(body)
                .map_err(|error| error.to_string())?,
        );
    }
    writer.seal().map_err(|error| error.to_string())?;
    let bytes = writer
        .into_inner()
        .map_err(|error| error.to_string())?
        .into_inner();
    Ok((bytes, ids))
}

fn build_mcap(data: &ProfileData) -> Result<Vec<u8>, String> {
    let options = WriteOptions::new()
        .profile(PROFILE)
        .library("alex-lar-container-gate")
        .compression(Some(Compression::Zstd))
        .compression_level(3)
        .compression_threads(0)
        .chunk_size(Some(1024 * 1024));
    let mut writer = Writer::with_options(Cursor::new(Vec::new()), options)
        .map_err(|error| error.to_string())?;
    let channel = writer
        .add_channel(
            0,
            MANIFEST_TOPIC,
            "application/vnd.alexandria.lar-manifest",
            &BTreeMap::new(),
        )
        .map_err(|error| error.to_string())?;
    let mut written_chunks = HashSet::new();
    for (index, manifest) in data.manifests.iter().enumerate() {
        for reference in &manifest.chunks {
            if !written_chunks.insert(reference.digest) {
                continue;
            }
            let bytes = data
                .chunks
                .get(&reference.digest)
                .ok_or("profile manifest names an unknown chunk")?;
            let compressed = zstd::bulk::compress(bytes, 3).map_err(|error| error.to_string())?;
            writer
                .attach(&Attachment {
                    log_time: index as u64 + 1,
                    create_time: index as u64 + 1,
                    name: format!("blake3:{}:{}", hex(&reference.digest), bytes.len()),
                    media_type: CHUNK_MEDIA_TYPE.into(),
                    data: Cow::Owned(compressed),
                })
                .map_err(|error| error.to_string())?;
        }
        let sequence = u32::try_from(index).map_err(|_| "too many profile manifests")?;
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: channel,
                    sequence,
                    log_time: index as u64 + 1,
                    publish_time: index as u64 + 1,
                },
                &encode_manifest(manifest),
            )
            .map_err(|error| error.to_string())?;
    }
    if written_chunks.len() != data.chunks.len() {
        return Err("profile contains an unreachable chunk".into());
    }
    writer.finish().map_err(|error| error.to_string())?;
    Ok(writer.into_inner().into_inner())
}

fn read_lar(bytes: &[u8], id: &ManifestId) -> Result<Vec<u8>, String> {
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default())
        .map_err(|error| error.to_string())?;
    reader.read_body(id).map_err(|error| error.to_string())
}

fn read_mcap(bytes: &[u8], ordinal: usize) -> Result<Vec<u8>, String> {
    let summary = Summary::read(bytes)
        .map_err(|error| error.to_string())?
        .ok_or("MCAP profile has no summary")?;
    let log_time = ordinal as u64 + 1;
    let chunk = summary
        .chunk_indexes
        .iter()
        .find(|index| index.message_start_time <= log_time && index.message_end_time >= log_time)
        .ok_or("MCAP profile cannot locate manifest chunk")?;
    let indexes = summary
        .read_message_indexes(bytes, chunk)
        .map_err(|error| error.to_string())?;
    let entry = indexes
        .iter()
        .find(|(channel, _)| channel.topic == MANIFEST_TOPIC)
        .and_then(|(_, entries)| entries.iter().find(|entry| entry.log_time == log_time))
        .ok_or("MCAP profile cannot locate manifest message")?;
    let message = summary
        .seek_message(bytes, chunk, entry)
        .map_err(|error| error.to_string())?;
    let manifest = decode_manifest(&message.data)?;
    let attachments = summary
        .attachment_indexes
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut body = Vec::with_capacity(manifest.total_length as usize);
    for reference in &manifest.chunks {
        if reference.logical_offset != body.len() as u64 {
            return Err("MCAP profile manifest has non-contiguous ranges".into());
        }
        let name = format!("blake3:{}:{}", hex(&reference.digest), reference.length);
        let index = attachments
            .get(name.as_str())
            .ok_or("MCAP profile chunk attachment is missing")?;
        let stored = attachment(bytes, index).map_err(|error| error.to_string())?;
        if stored.media_type != CHUNK_MEDIA_TYPE {
            return Err("MCAP profile chunk has wrong media type".into());
        }
        let chunk = zstd::bulk::decompress(&stored.data, reference.length as usize)
            .map_err(|error| error.to_string())?;
        if chunk.len() as u64 != reference.length
            || blake3::hash(&chunk).as_bytes() != &reference.digest
        {
            return Err("MCAP profile chunk integrity mismatch".into());
        }
        body.extend_from_slice(&chunk);
    }
    if body.len() as u64 != manifest.total_length
        || blake3::hash(&body).as_bytes() != &manifest.whole_hash
    {
        return Err("MCAP profile body integrity mismatch".into());
    }
    Ok(body)
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn recover_lar(bytes: &[u8]) -> usize {
    let cut = bytes.len() * 4 / 5;
    ArchiveReader::open(Cursor::new(&bytes[..cut]), Limits::default())
        .ok()
        .map(|reader| reader.manifest_ids().count())
        .unwrap_or(0)
}

fn recover_mcap(bytes: &[u8]) -> (usize, usize) {
    let cut = bytes.len() * 4 / 5;
    let mut attachments = 0;
    let mut messages = 0;
    let Ok(reader) =
        ChunkFlattener::new_with_options(&bytes[..cut], enum_set!(Options::IgnoreEndMagic))
    else {
        return (0, 0);
    };
    for record in reader {
        match record {
            Ok(Record::Attachment { .. }) => attachments += 1,
            Ok(Record::Message { .. }) => messages += 1,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    (attachments, messages)
}

fn parse_turns() -> Result<usize, String> {
    let mut args = std::env::args().skip(1);
    let mut turns = 77;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--turns" => {
                turns = args
                    .next()
                    .ok_or("--turns requires a value")?
                    .parse()
                    .map_err(|_| "--turns must be an integer")?;
            }
            "--help" | "-h" => return Err("usage: container_gate [--turns N]".into()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if turns == 0 || turns > u32::MAX as usize {
        return Err("--turns must be between 1 and u32::MAX".into());
    }
    Ok(turns)
}

fn run() -> Result<(), String> {
    let turns = parse_turns()?;
    let bodies = synthetic_bodies(turns);
    let profile = profile_data(&bodies)?;
    let (lar, ids) = build_lar(&bodies)?;
    let mcap = build_mcap(&profile)?;
    let target = turns - 1;
    if read_lar(&lar, &ids[target])? != bodies[target]
        || read_mcap(&mcap, target)? != bodies[target]
    {
        return Err("container reconstruction mismatch".into());
    }
    let lar_reads = (0..50)
        .map(|_| {
            let started = Instant::now();
            read_lar(&lar, &ids[target]).unwrap();
            started.elapsed()
        })
        .collect();
    let mcap_reads = (0..50)
        .map(|_| {
            let started = Instant::now();
            read_mcap(&mcap, target).unwrap();
            started.elapsed()
        })
        .collect();
    let lar_recovered = recover_lar(&lar);
    let (mcap_attachments, mcap_messages) = recover_mcap(&mcap);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "alex-lar-container-gate-v1",
            "turns": turns,
            "logical_body_bytes": bodies.iter().map(Vec::len).sum::<usize>(),
            "unique_chunks": profile.chunks.len(),
            "native_lar": {
                "bytes": lar.len(),
                "random_last_body_median_ns": median(lar_reads).as_nanos() as u64,
                "recoverable_manifests_at_80_percent_cut": lar_recovered,
            },
            "mcap_profile": {
                "profile": PROFILE,
                "bytes": mcap.len(),
                "random_last_body_median_ns": median(mcap_reads).as_nanos() as u64,
                "recoverable_attachments_at_80_percent_cut": mcap_attachments,
                "recoverable_manifest_messages_at_80_percent_cut": mcap_messages,
            },
            "interpretation": [
                "both outputs reconstruct the same selected body and store each unique compressed chunk once",
                "the MCAP profile requires Alex-specific attachment naming, manifest encoding, content indexes, and integrity rules",
                "MCAP message indexes locate manifests by timestamp/channel, while LAR indexes the stable manifest/content ID directly",
                "recovery counts are structural smoke evidence, not a full truncation-boundary conformance proof"
            ]
        }))
        .unwrap()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("container-gate benchmark: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_containers_preserve_the_one_copy_chunk_invariant_and_reconstruct() {
        let bodies = synthetic_bodies(14);
        let profile = profile_data(&bodies).unwrap();
        assert!(profile.chunks.len() < profile.manifests.iter().map(|m| m.chunks.len()).sum());
        let (lar, ids) = build_lar(&bodies).unwrap();
        let mcap = build_mcap(&profile).unwrap();
        for index in [0, 6, 13] {
            assert_eq!(read_lar(&lar, &ids[index]).unwrap(), bodies[index]);
            assert_eq!(read_mcap(&mcap, index).unwrap(), bodies[index]);
        }
    }

    #[test]
    fn profile_manifest_decoder_rejects_truncation_and_extra_bytes() {
        let profile = profile_data(&synthetic_bodies(1)).unwrap();
        let encoded = encode_manifest(&profile.manifests[0]);
        assert!(decode_manifest(&encoded[..encoded.len() - 1]).is_err());
        let mut extra = encoded;
        extra.push(0);
        assert!(decode_manifest(&extra).is_err());
    }
}
