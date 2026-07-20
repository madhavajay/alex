#[path = "../testsupport/golden.rs"]
mod golden;

use std::path::{Path, PathBuf};

fn write_or_verify(path: &Path, bytes: &[u8], write: bool) -> Result<(), String> {
    if write {
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| format!("{} has no parent", path.display()))?,
        )
        .map_err(|error| format!("creating {}: {error}", path.display()))?;
        std::fs::write(path, bytes)
            .map_err(|error| format!("writing {}: {error}", path.display()))?;
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
        return Ok(());
    }

    let existing = std::fs::read(path).map_err(|error| {
        format!(
            "cannot verify {}: {error}; rerun with --write to create fixtures",
            path.display()
        )
    })?;
    if existing != bytes {
        return Err(format!(
            "{} differs from deterministic output; inspect the format change, then rerun with --write",
            path.display()
        ));
    }
    println!("verified {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

fn main() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let write = match arguments.as_slice() {
        [] => false,
        [flag] if flag == "--write" => true,
        _ => {
            return Err(
                "usage: cargo run -p alex-lar --example generate_golden -- [--write]".into(),
            )
        }
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let full = golden::v1_0_full_archive();
    let future = golden::v1_future_minor_optional_archive();
    let conversation = golden::v1_conversation_dag_archive();
    let exchange_metadata = golden::v1_exchange_metadata_archive();
    for (relative, bytes) in [
        ("testdata/v1.0-full.lar", full.as_slice()),
        ("testdata/v1.future-minor-optional.lar", future.as_slice()),
        ("testdata/v1.conversation-dag.lar", conversation.as_slice()),
        (
            "testdata/v1.exchange-metadata.lar",
            exchange_metadata.as_slice(),
        ),
        ("fuzz/corpus/record_framing/v1.0-full.lar", full.as_slice()),
        (
            "fuzz/corpus/zstd_decompression/v1.0-full.lar",
            full.as_slice(),
        ),
        (
            "fuzz/corpus/metadata_indexes/v1.0-full.lar",
            full.as_slice(),
        ),
        (
            "fuzz/corpus/manifest_recovery/v1.0-full.lar",
            full.as_slice(),
        ),
    ] {
        write_or_verify(&root.join(relative), bytes, write)?;
    }
    if !write {
        println!("verification only; pass --write explicitly to overwrite fixtures");
    }
    Ok(())
}
