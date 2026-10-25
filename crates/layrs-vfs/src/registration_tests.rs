use super::*;
use crate::{ControlDirectoryIdentity, ControlDirectoryStageIdentity};

fn id(byte: char) -> FileId {
    FileId::parse(&format!("blake3:{}", byte.to_string().repeat(64))).unwrap()
}
fn control() -> ControlDirectoryIdentity {
    ControlDirectoryIdentity {
        stage: ControlDirectoryStageIdentity {
            root: "C:\\root".into(),
            parent: "C:\\private".into(),
            stage: "C:\\private\\stage".into(),
            volume: 0,
            root_index: 1,
            parent_index: 2,
            directory_index: 3,
            format_index: 4,
        },
        size: 3,
        content: FileId::from_bytes(b"fmt"),
    }
}
fn ticket() -> Registration {
    Registration {
        root: "C:\\root".into(),
        root_id: [0x11; 16],
        catalog_id: id('2'),
        volume: 0,
        file_index: 0x0102030405060708,
        flavor: RegistrationFlavor::ControlDirectory { digest: id('3') },
    }
}

#[test]
fn v1_wire_golden_and_v2_decoder_are_disjoint() {
    let mut value = ticket();
    value.flavor = RegistrationFlavor::Legacy;
    let mut golden = b"LYRTICK1".to_vec();
    golden.extend([0x11; 16]);
    golden.extend_from_slice(
        b"blake3:2222222222222222222222222222222222222222222222222222222222222222",
    );
    golden.extend([0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]);
    golden.extend_from_slice(b"C:\\root");
    assert_eq!(value.encode(), golden);
    assert_eq!(Registration::decode(&golden).unwrap().encode(), golden);
    assert!(Registration::decode_control_directory(&golden).is_err());
    assert!(Registration::decode(&ticket().encode()).is_err());
    assert!(value.matches_control(&control()).is_err());
}

#[test]
fn v2_ticket_and_root_blob_golden_offsets() {
    let value = ticket();
    let bytes = value.encode();
    let mut golden = b"LYRTICK2".to_vec();
    golden.extend([0x11; 16]);
    golden.extend_from_slice(
        b"blake3:2222222222222222222222222222222222222222222222222222222222222222",
    );
    golden.extend([0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]);
    golden.extend_from_slice(
        b"blake3:3333333333333333333333333333333333333333333333333333333333333333",
    );
    golden.extend([1, 0, 0, 0, 7, 0, 0, 0]);
    golden.extend_from_slice(b"C:\\root");
    assert_eq!(bytes, golden);
    assert_eq!(bytes.len(), 193);
    assert_eq!(
        Registration::decode_control_directory(&golden)
            .unwrap()
            .encode(),
        golden
    );
    for ready in [false, true] {
        let mut blob = b"LYRROOT2".to_vec();
        blob.push(u8::from(ready));
        blob.extend_from_slice(&golden[8..182]);
        assert_eq!(value.root_blob(ready), blob);
        assert_eq!(blob.len(), 183);
    }
}

#[test]
fn v2_strict_lengths_fields_revision_and_paths() {
    let valid = ticket().encode();
    for end in 0..valid.len() {
        assert!(
            Registration::decode_control_directory(&valid[..end]).is_err(),
            "cut {end}"
        );
    }
    let mut extra = valid.clone();
    extra.push(0);
    assert!(Registration::decode_control_directory(&extra).is_err());
    for (start, end) in [(8, 24), (99, 107)] {
        let mut bad = valid.clone();
        bad[start..end].fill(0);
        assert!(Registration::decode_control_directory(&bad).is_err());
    }
    for offset in [24, 94, 107, 177, 186] {
        let mut bad = valid.clone();
        bad[offset] = 0xff;
        assert!(Registration::decode_control_directory(&bad).is_err());
    }
    for revision in [0u32, 2, u32::MAX] {
        let mut bad = valid.clone();
        bad[178..182].copy_from_slice(&revision.to_le_bytes());
        assert!(matches!(
            Registration::decode_control_directory(&bad),
            Err(Error::Unsupported(_))
        ));
    }
    for root in [
        "relative",
        "C:\\..\\root",
        "C:\\root\\",
        "C:/root",
        "\\\\?\\C:\\root",
        "C:\\root:stream",
    ] {
        let mut bad = valid[..186].to_vec();
        bad[182..186].copy_from_slice(&(root.len() as u32).to_le_bytes());
        bad.extend(root.as_bytes());
        assert!(
            Registration::decode_control_directory(&bad).is_err(),
            "{root}"
        );
    }
    // Zero volume serial is supported; nonzero native index is the requirement.
    assert_eq!(
        Registration::decode_control_directory(&valid)
            .unwrap()
            .volume,
        0
    );
    for root in [
        format!("C:\\{}", "x".repeat(4079)),
        format!("C:\\{}", "é".repeat(2039)),
    ] {
        let mut candidate = ticket();
        candidate.root = root.into();
        assert!(Registration::decode_control_directory(&candidate.encode()).is_ok());
    }
    let mut too_long = ticket();
    too_long.root = format!("C:\\{}", "x".repeat(4080)).into();
    assert!(Registration::decode_control_directory(&too_long.encode()).is_err());
}

#[test]
fn v2_digest_binds_complete_historical_control_identity() {
    let value = control();
    let registration = Registration::for_control_directory(&value, [1; 16], id('2')).unwrap();
    let mut input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    input.extend(value.encode());
    assert_eq!(
        registration.flavor,
        RegistrationFlavor::ControlDirectory {
            digest: FileId::from_bytes(&input)
        }
    );
    registration.matches_control(&value).unwrap();
    let mut changed = value.clone();
    changed.stage.stage.push('x');
    assert!(registration.matches_control(&changed).is_err());
    changed = value.clone();
    changed.stage.format_index += 1;
    assert!(registration.matches_control(&changed).is_err());
    changed = value.clone();
    changed.content = FileId::from_bytes(b"different");
    assert!(registration.matches_control(&changed).is_err());
    assert!(matches!(
        registration.require_legacy(),
        Err(Error::Unsupported(_))
    ));
}

#[cfg(windows)]
#[test]
fn legacy_entries_refuse_v2_before_source_or_native_access() {
    use crate::{Cancellation, Catalog, Session, SnapshotSource};
    struct NeverSource;
    impl SnapshotSource for NeverSource {
        fn catalog(&self) -> &Catalog {
            panic!("legacy entry accessed V2 source")
        }
        fn read_exact_at(&self, _: &FileId, _: u64, _: &mut [u8], _: &Cancellation) -> Result<()> {
            panic!("legacy entry read V2 bytes")
        }
    }
    // Deliberately nonexistent root: the flavor refusal must win over native IO.
    let value = ticket();
    assert!(matches!(
        value.verify_root_identity(),
        Err(Error::Unsupported(_))
    ));
    let catalog = Catalog::new(1, vec![]).unwrap();
    assert!(matches!(value.verify(&catalog), Err(Error::Unsupported(_))));
    assert!(matches!(
        Session::reconnect(value, std::sync::Arc::new(NeverSource)),
        Err(Error::Unsupported(_))
    ));
}
