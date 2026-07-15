use super::*;
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
};

fn unique_store(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = env::temp_dir().join(format!("layrs-object-store-{name}-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn write_test_object(layrs_dir: &Path, bytes: &[u8]) -> FileObjectFile {
    let hash = blake3_id(bytes);
    write_file_object_manifest(layrs_dir, &hash, bytes, "text/plain").unwrap();
    let relative = PathBuf::from("objects")
        .join("files")
        .join(format!("{}.json", object_file_stem(&hash)));
    read_file_object_manifest(layrs_dir, &relative).unwrap()
}

fn loose_chunk_paths(layrs_dir: &Path) -> Vec<PathBuf> {
    let chunks_dir = layrs_dir.join("objects").join("chunks");
    if !chunks_dir.exists() {
        return Vec::new();
    }
    fs::read_dir(chunks_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("chunk"))
        .collect()
}

fn temp_pack_artifacts(layrs_dir: &Path) -> Vec<PathBuf> {
    let packs_dir = layrs_dir.join("objects").join("packs");
    if !packs_dir.exists() {
        return Vec::new();
    }
    fs::read_dir(packs_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.starts_with("tmp-"))
        })
        .collect()
}

fn published_pack_artifacts(layrs_dir: &Path) -> Vec<PathBuf> {
    let packs_dir = layrs_dir.join("objects").join("packs");
    if !packs_dir.exists() {
        return Vec::new();
    }
    fs::read_dir(packs_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("pack" | "json")
            )
        })
        .collect()
}

#[test]
fn corrupt_temp_pack_is_rejected_even_while_loose_chunk_is_readable() {
    let layrs_dir = unique_store("corrupt-pack");
    let expected = "pack verification must not read loose bytes\n".repeat(8_000);
    let manifest = write_test_object(&layrs_dir, expected.as_bytes());
    let loose_before = loose_chunk_paths(&layrs_dir);
    assert!(!loose_before.is_empty());

    let result = compact_loose_chunks_with_hook(&layrs_dir, |pack_path, _| {
        let mut pack = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pack_path)
            .map_err(|error| error.to_string())?;
        pack.seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        pack.write_all(b"corrupt")
            .map_err(|error| error.to_string())?;
        pack.sync_all().map_err(|error| error.to_string())
    });

    assert!(result.is_err());
    assert!(loose_before.iter().all(|path| path.exists()));
    assert_eq!(
        read_file_object_bytes(&layrs_dir, &manifest).unwrap(),
        expected.as_bytes()
    );
    assert!(temp_pack_artifacts(&layrs_dir).is_empty());
    assert!(published_pack_artifacts(&layrs_dir).is_empty());
    let _ = fs::remove_dir_all(layrs_dir);
}

#[test]
fn corrupt_temp_index_is_rejected_without_removing_loose_chunks() {
    let layrs_dir = unique_store("corrupt-index");
    let expected = "index ranges must be verified\n".repeat(6_000);
    let manifest = write_test_object(&layrs_dir, expected.as_bytes());
    let loose_before = loose_chunk_paths(&layrs_dir);

    let result = compact_loose_chunks_with_hook(&layrs_dir, |_, index_path| {
        let mut index = read_json::<ChunkPackIndex>(index_path)?;
        index.entries[0].offset = u64::MAX;
        fs::write(
            index_path,
            serde_json::to_vec_pretty(&index).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    });

    assert!(result.is_err());
    assert!(loose_before.iter().all(|path| path.exists()));
    assert_eq!(
        read_file_object_bytes(&layrs_dir, &manifest).unwrap(),
        expected.as_bytes()
    );
    assert!(temp_pack_artifacts(&layrs_dir).is_empty());
    assert!(published_pack_artifacts(&layrs_dir).is_empty());
    let _ = fs::remove_dir_all(layrs_dir);
}

#[test]
fn incomplete_temp_pack_artifacts_are_ignored() {
    let layrs_dir = unique_store("incomplete-temp");
    let expected = "stale temp files are not published indexes\n".repeat(4_000);
    let manifest = write_test_object(&layrs_dir, expected.as_bytes());
    let packs_dir = layrs_dir.join("objects").join("packs");
    fs::create_dir_all(&packs_dir).unwrap();
    let stale_pack = packs_dir.join("interrupted.pack.tmp-old");
    let stale_index = packs_dir.join("interrupted.json.tmp-old");
    fs::write(&stale_pack, b"partial pack").unwrap();
    fs::write(&stale_index, b"partial index").unwrap();

    let compacted = compact_loose_chunks(&layrs_dir).unwrap();

    assert!(compacted.packed_chunks > 0);
    assert!(stale_pack.exists());
    assert!(stale_index.exists());
    assert_eq!(
        read_file_object_bytes(&layrs_dir, &manifest).unwrap(),
        expected.as_bytes()
    );
    let _ = fs::remove_dir_all(layrs_dir);
}

#[test]
fn verified_pack_remains_readable_after_loose_chunks_are_removed() {
    let layrs_dir = unique_store("verified-pack");
    let expected = "verified pack survives loose cleanup\n".repeat(10_000);
    let manifest = write_test_object(&layrs_dir, expected.as_bytes());
    assert!(!loose_chunk_paths(&layrs_dir).is_empty());

    let compacted = compact_loose_chunks(&layrs_dir).unwrap();

    assert!(compacted.packed_chunks > 0);
    assert!(compacted.loose_chunks_removed > 0);
    assert!(loose_chunk_paths(&layrs_dir).is_empty());
    assert_eq!(
        read_file_object_bytes(&layrs_dir, &manifest).unwrap(),
        expected.as_bytes()
    );
    let _ = fs::remove_dir_all(layrs_dir);
}

#[test]
fn legacy_identity_chunk_without_metadata_survives_compaction() {
    let layrs_dir = unique_store("legacy-identity");
    let expected = b"legacy identity chunk bytes".repeat(2_000);
    let chunk_id = blake3_id(&expected);
    let chunk_path = loose_chunk_path(&layrs_dir, &chunk_id);
    fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    fs::write(&chunk_path, &expected).unwrap();

    assert_eq!(
        read_chunk_raw(&layrs_dir, &chunk_id, None).unwrap(),
        expected
    );
    let compacted = compact_loose_chunks(&layrs_dir).unwrap();

    assert_eq!(compacted.packed_chunks, 1);
    assert_eq!(compacted.loose_chunks_removed, 1);
    assert!(!chunk_path.exists());
    assert_eq!(
        read_chunk_raw(&layrs_dir, &chunk_id, None).unwrap(),
        expected
    );
    let _ = fs::remove_dir_all(layrs_dir);
}
