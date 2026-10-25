//! Opt-in owned fixtures; no production Store receipts or automatic cleanup.
use super::*;
use crate::*;
#[path = "control_stage_observation_fixture.rs"]
mod fixture;
#[path = "control_stage_observation_native.rs"]
mod native;
#[path = "control_stage_observation_registration.rs"]
mod registration;
use fixture::{FORMAT, Fixture};

#[test]
fn allocated_observed_identity_compares_every_field_and_case() {
    let expected = ControlDirectoryStageIdentity {
        root: "C:\\Owned\\Root".into(),
        parent: "C:\\Owned\\Parent".into(),
        stage: "C:\\Owned\\Parent\\Stage".into(),
        volume: 0,
        root_index: 1,
        parent_index: 2,
        directory_index: 3,
        format_index: 4,
    };
    expected.validate().unwrap();
    let identity = ObservedAllocatedControlStageIdentity(&expected);
    assert!(identity.matches_expected(&expected));
    for field in 0..8 {
        let mut changed = expected.clone();
        match field {
            0 => changed.root.push('x'),
            1 => changed.parent.push('x'),
            2 => changed.stage.push('x'),
            3 => changed.volume = 1,
            4 => changed.root_index = 5,
            5 => changed.parent_index = 5,
            6 => changed.directory_index = 5,
            7 => changed.format_index = 5,
            _ => unreachable!(),
        }
        assert!(!identity.matches_expected(&changed), "field {field}");
    }
    for field in 0..3 {
        let mut changed = expected.clone();
        match field {
            0 => changed.root = changed.root.to_lowercase(),
            1 => changed.parent = changed.parent.to_lowercase(),
            2 => changed.stage = changed.stage.to_lowercase(),
            _ => unreachable!(),
        }
        assert!(!identity.matches_expected(&changed), "case field {field}");
    }
}

#[test]
fn final_absence_accepts_only_raw_file_not_found() {
    absence_result::<()>(Err(std::io::Error::from_raw_os_error(2).into())).unwrap();
    for raw in [3, 5, 32, 80, 183, 303, 389] {
        let error = absence_result::<()>(Err(std::io::Error::from_raw_os_error(raw).into()));
        assert!(matches!(error, Err(Error::Io(error)) if error.raw_os_error() == Some(raw)));
    }
    assert!(matches!(absence_result(Ok(())), Err(Error::ForeignRoot)));
    assert!(matches!(
        absence_result::<()>(Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "untyped"
        )))),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        absence_result::<()>(Err(Error::Windows {
            operation: "test",
            hresult: 0x80070002u32 as i32
        })),
        Err(Error::Windows { .. })
    ));
}

#[test]
fn allocated_observation_rejects_invalid_bounds_before_native_access() {
    for path in [
        "relative".to_owned(),
        "C:\\".to_owned(),
        format!("C:\\{}", "a".repeat(4096)),
        format!("C:\\{}", "é".repeat(2048)),
    ] {
        let expected = ControlDirectoryStageIdentity {
            root: path,
            parent: "C:\\owned\\parent".into(),
            stage: "C:\\owned\\parent\\stage".into(),
            volume: 0,
            root_index: 1,
            parent_index: 2,
            directory_index: 3,
            format_index: 4,
        };
        assert!(matches!(
            observe_allocated_control_stage(&expected),
            Err(Error::InvalidRoot(_))
        ));
    }
}
