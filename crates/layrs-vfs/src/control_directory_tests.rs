use super::*;

fn stage() -> ControlDirectoryStageIdentity {
    ControlDirectoryStageIdentity {
        root: r"C:\r".into(),
        parent: r"C:\p".into(),
        stage: r"C:\p\s".into(),
        volume: 0x12345678,
        root_index: 0x100000002,
        parent_index: 3,
        directory_index: 4,
        format_index: 0xa0000000b,
    }
}
fn golden_stage() -> Vec<u8> {
    [
        b"LYRCSTG1\x78\x56\x34\x12".as_slice(),
        b"\x02\0\0\0\x01\0\0\0\x03\0\0\0\0\0\0\0",
        b"\x04\0\0\0\0\0\0\0\x0b\0\0\0\x0a\0\0\0",
        b"\x04\0\0\0C:\\r\x04\0\0\0C:\\p\x06\0\0\0C:\\p\\s",
    ]
    .concat()
}
fn identity() -> ControlDirectoryIdentity {
    ControlDirectoryIdentity {
        stage: stage(),
        size: 7,
        content: FileId::parse(&format!("blake3:{}", "0".repeat(64))).unwrap(),
    }
}
#[test]
fn control_directory_codecs_and_identity_encodings_are_golden() {
    let stage = stage();
    assert_eq!(golden_stage().len(), 70);
    assert_eq!(stage.encode(), golden_stage());
    assert_eq!(
        ControlDirectoryStageIdentity::decode(&golden_stage()).unwrap(),
        stage
    );
    assert_eq!(
        stage.root_filesystem_identity(),
        "fs:v1:0000000012345678:0000000100000002"
    );
    assert_eq!(
        stage.stage_parent_filesystem_identity(),
        "fs:v1:0000000012345678:0000000000000003"
    );
    assert_eq!(
        stage.directory_filesystem_identity(),
        "fs:v1:0000000012345678:0000000000000004"
    );
    assert_eq!(stage.format_binding_identity(), "305419896:10:11");
    let expected = [
        b"LYRCDIR1\x46\0\0\0".as_slice(),
        &golden_stage(),
        b"\x07\0\0\0",
        b"blake3:0000000000000000000000000000000000000000000000000000000000000000",
    ]
    .concat();
    assert_eq!(identity().encode(), expected);
    assert_eq!(
        ControlDirectoryIdentity::decode(&expected).unwrap(),
        identity()
    );
    assert_eq!(CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES, 4096);
    assert_eq!(CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES, 12344);
    assert_eq!(CONTROL_DIRECTORY_MAX_ENCODED_BYTES, 12431);
    assert_eq!(CONTROL_DIRECTORY_MAX_FORMAT_BYTES, 65536);
}
#[test]
fn control_directory_decoders_refuse_short_trailing_and_overflow_records() {
    let stage = golden_stage();
    for end in 0..stage.len() {
        assert!(
            ControlDirectoryStageIdentity::decode(&stage[..end]).is_err(),
            "stage prefix {end}"
        );
    }
    let encoded = identity().encode();
    for end in 0..encoded.len() {
        assert!(
            ControlDirectoryIdentity::decode(&encoded[..end]).is_err(),
            "identity prefix {end}"
        );
    }
    let mut extra = encoded.clone();
    extra.push(0);
    assert!(ControlDirectoryIdentity::decode(&extra).is_err());
    for size in [0u32, 65537, u32::MAX] {
        let mut bad = encoded.clone();
        bad[82..86].copy_from_slice(&size.to_le_bytes());
        assert!(ControlDirectoryIdentity::decode(&bad).is_err());
    }
    let mut bad = stage.clone();
    bad[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ControlDirectoryStageIdentity::decode(&bad).is_err());
    let mut bad = encoded;
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ControlDirectoryIdentity::decode(&bad).is_err());
    assert!(ControlDirectoryIdentity::decode(&vec![0; MAX_IDENTITY_BYTES + 1]).is_err());
}
#[test]
fn control_stage_data_refuses_aliases_invalid_utf8_and_unrelated_parent() {
    // Volume roots are pinned as ancestors but not admitted as allocation
    // parent targets by either this codec or the native RootPins constructor.
    let mut volume_parent = stage();
    volume_parent.parent = "C:\\".into();
    volume_parent.stage = "C:\\s".into();
    assert!(ControlDirectoryStageIdentity::decode(&volume_parent.encode()).is_err());
    for path in [
        r"C:relative",
        r"\\server\share",
        r"\\?\C:\root",
        r"C:\r\..\s",
        r"C:\r\x.",
        r"C:\r\CON.txt",
        r"C:\r\file:stream",
        "C:\\r\0x",
        "C:/root",
        "C:\\r\\",
    ] {
        let mut value = stage();
        value.root = path.into();
        assert!(
            ControlDirectoryStageIdentity::decode(&value.encode()).is_err(),
            "{path:?}"
        );
    }
    let mut value = stage();
    value.stage = r"C:\other\s".into();
    assert!(ControlDirectoryStageIdentity::decode(&value.encode()).is_err());
    let mut value = stage();
    value.format_index = value.directory_index;
    assert!(ControlDirectoryStageIdentity::decode(&value.encode()).is_err());
    let mut bytes = golden_stage();
    bytes[51] = 0xff;
    assert!(ControlDirectoryStageIdentity::decode(&bytes).is_err());
    let mut value = stage();
    value.root = format!("C:\\{}", "é".repeat(2047));
    assert!(value.root.len() > MAX_PATH_BYTES);
    assert!(ControlDirectoryStageIdentity::decode(&value.encode()).is_err());
}
#[test]
fn observed_control_identity_compares_physical_fields_only_and_is_not_expected_data() {
    let bytes = b"opaque FORMAT".to_vec();
    let mut expected = identity();
    expected.size = bytes.len() as u32;
    expected.content = FileId::from_bytes(&bytes);
    let observed = ObservedControlDirectory {
        root: expected.stage.root.clone(),
        volume: expected.stage.volume,
        root_index: expected.stage.root_index,
        directory_index: expected.stage.directory_index,
        format_index: expected.stage.format_index,
        bytes,
        content: expected.content.clone(),
    };
    assert!(observed.identity().matches_expected(&expected));
    let mut changed = expected.clone();
    changed.stage.format_index += 1;
    assert!(!observed.identity().matches_expected(&changed));
    let mut historical = expected;
    historical.stage.parent_index += 100;
    // The caller authenticates history. Discovery cannot attest an allocation.
    assert!(observed.identity().matches_expected(&historical));
}

fn path_of_utf8_length(length: usize, multibyte: bool) -> String {
    let mut path = r"C:\r".to_owned();
    let component = if multibyte { "\\é" } else { "\\a" };
    while path.len() + component.len() + 1 <= length {
        path.push_str(component);
    }
    path.push_str(&"x".repeat(length - path.len()));
    assert_eq!(path.len(), length);
    if multibyte {
        assert!(path.chars().count() < length);
    }
    path
}

#[test]
fn fixed_control_suffixes_fit_before_preflight_or_publication() {
    for multibyte in [false, true] {
        let maximum_root = MAX_PATH_BYTES - "\\.layrs\\FORMAT".len();
        let root = path_of_utf8_length(maximum_root, multibyte);
        assert!(validate_control_paths(&root).is_ok());
        assert_eq!(format!("{root}\\.layrs\\FORMAT").len(), MAX_PATH_BYTES);
        let too_long = path_of_utf8_length(maximum_root + 1, multibyte);
        assert!(validate_path(&too_long).is_ok(), "base alone still fits");
        assert!(validate_control_paths(&too_long).is_err());
        let mut allocation = stage();
        allocation.root = too_long.clone();
        assert!(ControlDirectoryStageIdentity::decode(&allocation.encode()).is_err());
        #[cfg(windows)]
        assert!(
            matches!(
                crate::ControlRootPreflight::open(Path::new(&too_long)),
                Err(Error::InvalidRoot(_))
            ),
            "derived path must refuse before opening the nonexistent root"
        );

        let maximum_stage = MAX_PATH_BYTES - "\\FORMAT".len();
        assert!(validate_stage_paths(&path_of_utf8_length(maximum_stage, multibyte)).is_ok());
        let too_long_stage = path_of_utf8_length(maximum_stage + 1, multibyte);
        assert!(validate_path(&too_long_stage).is_ok());
        assert!(validate_stage_paths(&too_long_stage).is_err());
        let mut allocation = stage();
        allocation.parent = too_long_stage.rsplit_once('\\').unwrap().0.to_owned();
        allocation.stage = too_long_stage;
        assert!(
            ControlDirectoryStageIdentity::decode(&allocation.encode()).is_err(),
            "allocation must reject stage/FORMAT overflow before publisher effects"
        );
    }
}
