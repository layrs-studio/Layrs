//! Current-policy native registration/counter fixtures. Registrations retained.
use super::*;
use std::{
    mem::{offset_of, size_of},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::{Win32::Storage::CloudFilters::*, core::GUID};

pub(super) fn register(path: &Path, label: &str) {
    let api = Api::load().unwrap();
    let name: Vec<_> = "Layrs observation qualification\0".encode_utf16().collect();
    let version = [b'1' as u16, 0];
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: label.as_ptr().cast(),
        SyncRootIdentityLength: label.len() as u32,
        FileIdentity: label.as_ptr().cast(),
        FileIdentityLength: label.len() as u32,
        ProviderId: GUID::from_u128(0x3290815c_379d_4fa1_817c_8a400a9854e1),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_PROGRESSIVE,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_TRACK_ALL,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    let hr = unsafe {
        (api.register)(
            fs::native_path(path).unwrap().as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
        )
    };
    eprintln!(
        "ACTUAL observation fixture CfRegister {}: 0x{:08x}; registration retained",
        path.display(),
        hr as u32
    );
    assert_eq!(hr, 0, "UNQUALIFIED native registration {label}");
}
struct Counts {
    api: Api,
    data: AtomicU64,
    names: AtomicU64,
    notifications: AtomicU64,
    malformed: AtomicU64,
}
static BAD: AtomicU64 = AtomicU64::new(0);
unsafe fn context(info: *const CF_CALLBACK_INFO) -> Option<(CF_CALLBACK_INFO, &'static Counts)> {
    if info.is_null()
        || !(size_of::<CF_CALLBACK_INFO>() as u32..=65536)
            .contains(&unsafe { info.cast::<u32>().read_unaligned() })
    {
        BAD.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    let info = unsafe { info.read_unaligned() };
    if info.CallbackContext.is_null() {
        BAD.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    Some((info, unsafe { &*info.CallbackContext.cast::<Counts>() }))
}
unsafe extern "system" fn notification(
    info: *const CF_CALLBACK_INFO,
    _: *const CF_CALLBACK_PARAMETERS,
) {
    if let Some((_, counts)) = unsafe { context(info) } {
        counts.notifications.fetch_add(1, Ordering::SeqCst);
    }
}
unsafe fn refuse(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
    names: bool,
) {
    let Some((info, counts)) = (unsafe { context(info) }) else {
        return;
    };
    let mut operation = CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        RequestKey: info.RequestKey,
        ..Default::default()
    };
    let mut output = CF_OPERATION_PARAMETERS::default();
    if names {
        counts.names.fetch_add(1, Ordering::SeqCst);
        output.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_4>()) as u32;
        output.Anonymous.TransferPlaceholders = CF_OPERATION_PARAMETERS_0_4 {
            CompletionStatus: windows_sys::Win32::Foundation::STATUS_CLOUD_FILE_NOT_SUPPORTED,
            ..Default::default()
        };
    } else {
        counts.data.fetch_add(1, Ordering::SeqCst);
        let end = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
            + offset_of!(CF_CALLBACK_PARAMETERS_0_1, RequiredLength)
            + size_of::<i64>();
        if params.is_null() || unsafe { params.cast::<u32>().read_unaligned() } < end as u32 {
            counts.malformed.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let offset = unsafe {
            std::ptr::addr_of!((*params).Anonymous.FetchData.RequiredFileOffset).read_unaligned()
        };
        let length = unsafe {
            std::ptr::addr_of!((*params).Anonymous.FetchData.RequiredLength).read_unaligned()
        };
        operation.Type = CF_OPERATION_TYPE_TRANSFER_DATA;
        output.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32;
        output.Anonymous.TransferData = CF_OPERATION_PARAMETERS_0_0 {
            CompletionStatus: windows_sys::Win32::Foundation::STATUS_CLOUD_FILE_NOT_SUPPORTED,
            Offset: offset,
            Length: length,
            ..Default::default()
        };
    }
    let hr = unsafe { (counts.api.execute)(&operation, &mut output) };
    eprintln!(
        "unexpected observation fetch completion: 0x{:08x}",
        hr as u32
    );
}
unsafe extern "system" fn data(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse(info, params, false) };
}
unsafe extern "system" fn names(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse(info, params, true) };
}
pub(super) fn connected(root: &Path, action: impl FnOnce()) {
    // Context/DLL remain valid even if assertions or disconnect fail.
    let counts = Box::leak(Box::new(Counts {
        api: Api::load().unwrap(),
        data: AtomicU64::new(0),
        names: AtomicU64::new(0),
        notifications: AtomicU64::new(0),
        malformed: AtomicU64::new(0),
    }));
    let callbacks = [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(data),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
            Callback: Some(names),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_OPEN_COMPLETION,
            Callback: Some(notification),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
            Callback: Some(notification),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ];
    let mut key = 0;
    assert_eq!(
        unsafe {
            (counts.api.connect)(
                fs::native_path(root).unwrap().as_ptr(),
                callbacks.as_ptr(),
                (counts as *const Counts).cast(),
                CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION,
                &mut key,
            )
        },
        0
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    let disconnect = unsafe { (counts.api.disconnect)(key) };
    let data = counts.data.load(Ordering::SeqCst);
    let names = counts.names.load(Ordering::SeqCst);
    eprintln!(
        "ACTUAL current-policy ordinary-control counters: data={data}, names={names}, notifications={}, malformed={}, disconnect=0x{:08x}; non-discriminating ALWAYS_FULL/DISABLE_ON_DEMAND_ROOT, no positive control or arbitrary-provider claim",
        counts.notifications.load(Ordering::SeqCst),
        counts.malformed.load(Ordering::SeqCst),
        disconnect as u32
    );
    assert_eq!(disconnect, 0);
    assert_eq!(BAD.load(Ordering::SeqCst), 0);
    assert_eq!(counts.malformed.load(Ordering::SeqCst), 0);
    assert_eq!((data, names), (0, 0));
    assert_eq!(counts.notifications.load(Ordering::SeqCst), 0);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn registered_refusal(label: &str, error: &Error) {
    let mut cause = error;
    while let Error::ControlDirectory { cause: inner, .. } = cause {
        cause = inner;
    }
    assert!(
        matches!(
            cause,
            Error::Windows {
                operation: "CfGetSyncRootInfoByHandle (expected NOT_UNDER_SYNC_ROOT)",
                hresult: 0,
            }
        ),
        "UNQUALIFIED {label}: expected positive native registration query refused by NOT_UNDER boundary; got {error:?}"
    );
}

#[test]
#[ignore = "owned registrations and current-policy connected metadata counter gate; fixtures retained"]
fn control_observation_registration_boundaries() {
    let run = fixture::run("registrations");
    for location in ["root", "parent", "stage"] {
        let f = Fixture::new(&run, &format!("allocated-{location}"), FORMAT);
        let target = match location {
            "root" => f.root.clone(),
            "parent" => f.allocated.stage_parent().to_path_buf(),
            "stage" => f.stage.clone(),
            _ => unreachable!(),
        };
        register(&target, location);
        let error = fixture::error(observe_allocated_control_stage(&f.allocated));
        registered_refusal(location, &error);
        eprintln!("PASS allocated {location} registered refusal: {error:?}");
    }
    for location in ["root", "ancestor", "control"] {
        let f = Fixture::new(&run, &format!("published-{location}"), FORMAT);
        let expected = f.publish().identity().clone();
        let guard = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
        guard.revalidate_unregistered_root().unwrap();
        let target = match location {
            "root" => f.root.clone(),
            "ancestor" => f.base.clone(),
            "control" => f.root.join(".layrs"),
            _ => unreachable!(),
        };
        register(&target, location); // actual mutation AFTER the expected-open/held proof
        let error = fixture::error(guard.revalidate_unregistered_root());
        assert!(matches!(
            &error,
            Error::ControlDirectory {
                operation: "revalidate unregistered root",
                ..
            }
        ));
        registered_refusal(location, &error);
        eprintln!("PASS published {location} registered refusal: {error:?}");
    }
    let f = Fixture::new(&run, "unrelated-descendant", FORMAT);
    let guard = f.publish();
    let unrelated = f.root.join("unrelated-user-directory");
    std::fs::create_dir(&unrelated).unwrap();
    register(&unrelated, "unrelated user descendant");
    guard.revalidate_unregistered_root().unwrap();
    assert!(guard.revalidate_bootstrap_root().is_err());
    eprintln!("PASS B ignores unrelated descendant registration: no recursive absence claim");

    let f = Fixture::new(&run, "registration-at-final-boundary", FORMAT);
    let root = f.root.clone();
    let injected = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = injected.clone();
    let registered = std::rc::Rc::new(std::cell::Cell::new(false));
    let completed = registered.clone();
    let result = fault::scoped(
        Box::new(move |point, _| {
            if point == fault::Point::BeforeFinalBoundary && !seen.replace(true) {
                register(&root, "late ordinary-root registration");
                completed.set(true);
            }
            Ok(())
        }),
        || observe_allocated_control_stage(&f.allocated),
    );
    assert!(
        injected.get(),
        "designated BeforeFinalBoundary was not reached"
    );
    assert!(
        registered.get(),
        "designated boundary did not complete successful registration"
    );
    let error = fixture::error(result);
    registered_refusal("late root registration at BeforeFinalBoundary", &error);
    eprintln!("PASS actual root registration at last observation boundary refused: {error:?}");

    let f = Fixture::new(&run, "connected", FORMAT);
    let expected = f.publish().identity().clone(); // temporary owner dropped before fresh open
    register(&f.root, "connected ordinary control");
    connected(&f.root, || {
        let guard = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
        guard.revalidate().unwrap();
        let error = fixture::error(guard.revalidate_unregistered_root());
        registered_refusal("connected published root B", &error);
        // This A call is already past publication; its refusal is not evidence
        // for allocated-stage registration coverage above.
        assert!(observe_allocated_control_stage(&f.allocated).is_err());
    });
}

// Independent targets below preserve the original aggregate, including its
// query-only expectation at a type-first boundary, as a separate failed recipe.
pub(super) fn registered_pair(label: &str, path: &Path, held: &File, before: io::Id) -> File {
    let fresh = io::directory_observer(path).unwrap();
    let api = Api::load().unwrap();
    for (which, file) in [("held", held), ("fresh", &fresh)] {
        assert_eq!(io::id(file).unwrap(), before, "{label} {which} native ID");
        let attributes = fs::attributes(file).unwrap(); // thread-scoped EXPOSE
        assert_ne!(attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY, 0);
        assert_ne!(attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
        assert_eq!(attributes.ReparseTag & 0xffff0fff, 0x9000001a);
        let mut basic = CF_SYNC_ROOT_BASIC_INFO::default();
        let hr = unsafe {
            (api.root_info)(
                fs::handle(file),
                CF_SYNC_ROOT_INFO_BASIC,
                (&mut basic as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
                size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
                std::ptr::null_mut(),
            )
        };
        eprintln!(
            "ACTUAL separate positive CF witness {label}/{which}: id={before:?}, attributes=0x{:08x}, tag=0x{:08x}, query=0x{:08x}, SyncRootFileId={:016x}; not production query coverage",
            attributes.FileAttributes,
            attributes.ReparseTag,
            hr as u32,
            basic.SyncRootFileId as u64
        );
        assert_eq!(hr, 0, "UNQUALIFIED {label}/{which} positive CF query");
        assert_eq!(basic.SyncRootFileId as u64, before.1);
        let error = fixture::error(io::not_under(&api, file));
        registered_refusal("separate test-only NOT_UNDER positive control", &error);
    }
    fresh
}

fn b_cause<'a>(f: &Fixture, error: &'a Error, operation: &'static str) -> &'a Error {
    match error {
        Error::ControlDirectory {
            kind: ControlDirectoryFailure::NeedsRepair,
            point: ControlDirectoryPublishPoint::BeforeProof,
            operation: actual,
            root,
            stage,
            cause,
        } => {
            assert_eq!(*actual, operation, "{error:?}");
            assert_eq!(root, &f.root);
            assert_eq!(stage, f.allocated.stage());
            cause
        }
        _ => panic!("UNQUALIFIED exact B wrapper required: {error:?}"),
    }
}

fn ordinary_type_refusal(label: &str, error: &Error) {
    assert!(
        matches!(
            error,
            Error::Unsupported("ordinary control directory required")
        ),
        "UNQUALIFIED {label}: expected exact ordinary-type refusal; got {error:?}"
    );
    eprintln!(
        "PASS {label}: TYPE FILTER ONLY; separate native CF witness does not establish production NOT_UNDER query coverage"
    );
}

fn allocated_registered_type_filter(location: &str) {
    let run = fixture::run(&format!("a-registered-{location}"));
    let f = Fixture::new(&run, location, FORMAT);
    let (path, expected_id) = match location {
        "root" => (
            f.root.as_path(),
            (f.allocated.volume, f.allocated.root_index),
        ),
        "parent" => (
            f.allocated.stage_parent(),
            (f.allocated.volume, f.allocated.parent_index),
        ),
        _ => unreachable!(),
    };
    let held = io::directory_observer(path).unwrap();
    io::plain_directory(&held).unwrap();
    assert_eq!(io::id(&held).unwrap(), expected_id);
    register(path, location);
    let _fresh = registered_pair(location, path, &held, expected_id);
    let error = fixture::error(observe_allocated_control_stage(&f.allocated));
    ordinary_type_refusal(location, &error);
    fixture::unchanged_stage(&f, FORMAT);
}

#[test]
#[ignore = "fresh registered A root; exact type filter only with independent positive CF witness"]
fn control_observation_a_registered_root() {
    allocated_registered_type_filter("root");
}

#[test]
#[ignore = "fresh registered A parent; exact type filter only with independent positive CF witness"]
fn control_observation_a_registered_parent() {
    allocated_registered_type_filter("parent");
}

#[test]
#[ignore = "fresh registered A stage; exact production NOT_UNDER query refusal"]
fn control_observation_a_registered_stage() {
    let run = fixture::run("a-registered-stage");
    let f = Fixture::new(&run, "stage", FORMAT);
    let held = io::directory_observer(&f.stage).unwrap();
    io::plain_directory(&held).unwrap();
    let expected_id = (f.allocated.volume, f.allocated.directory_index);
    assert_eq!(io::id(&held).unwrap(), expected_id);
    register(&f.stage, "allocated stage");
    let _fresh = registered_pair("allocated stage", &f.stage, &held, expected_id);
    let error = fixture::error(observe_allocated_control_stage(&f.allocated));
    registered_refusal("A stage production query", &error);
    assert!(matches!(&error, Error::Windows { .. }));
    fixture::unchanged_stage(&f, FORMAT);
    eprintln!("PASS A registered stage: exact production query refusal {error:?}");
}

fn published_registered_refusal(location: &str) {
    let run = fixture::run(&format!("b-registered-{location}"));
    let f = Fixture::new(&run, location, FORMAT);
    let expected = f.publish().identity().clone();
    let guard = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
    guard.revalidate_unregistered_root().unwrap();
    let path = match location {
        "root" => f.root.clone(),
        "control" => f.root.join(".layrs"),
        "ancestor" => f.base.clone(),
        _ => unreachable!(),
    };
    let held = io::directory_observer(&path).unwrap();
    io::plain_directory(&held).unwrap();
    let before = io::id(&held).unwrap();
    register(&path, location); // after the actual layout guard was acquired
    let _fresh = registered_pair(location, &path, &held, before);
    let error = fixture::error(guard.revalidate_unregistered_root());
    let cause = b_cause(&f, &error, "revalidate unregistered root");
    if location == "ancestor" {
        ordinary_type_refusal("B ancestor", cause);
    } else {
        registered_refusal(location, cause);
        assert!(matches!(cause, Error::Windows { .. }));
        eprintln!("PASS B registered {location}: exact wrapped production query refusal {error:?}");
    }
    assert_eq!(std::fs::read(f.root.join(".layrs/FORMAT")).unwrap(), FORMAT);
}

#[test]
#[ignore = "fresh registered B root; exact wrapper and production NOT_UNDER query refusal"]
fn control_observation_b_registered_root() {
    published_registered_refusal("root");
}

#[test]
#[ignore = "fresh registered B control; exact wrapper and production NOT_UNDER query refusal"]
fn control_observation_b_registered_control() {
    published_registered_refusal("control");
}

#[test]
#[ignore = "fresh registered B ancestor; exact wrapped type filter only with independent positive CF witness"]
fn control_observation_b_registered_ancestor() {
    published_registered_refusal("ancestor");
}

#[test]
#[ignore = "B remains nonrecursive; unrelated registered descendant allowed, exact bootstrap layout refusal"]
fn control_observation_b_unrelated_registered_descendant() {
    let run = fixture::run("b-unrelated-registration");
    let f = Fixture::new(&run, "unrelated", FORMAT);
    let guard = f.publish();
    let path = f.root.join("unrelated-user-directory");
    std::fs::create_dir(&path).unwrap();
    let held = io::directory_observer(&path).unwrap();
    io::plain_directory(&held).unwrap();
    let before = io::id(&held).unwrap();
    register(&path, "unrelated user descendant");
    let _fresh = registered_pair("unrelated user descendant", &path, &held, before);
    guard.revalidate().unwrap();
    guard.revalidate_unregistered_root().unwrap();
    let error = fixture::error(guard.revalidate_bootstrap_root());
    let cause = b_cause(&f, &error, "revalidate bootstrap root");
    assert!(
        matches!(
            cause,
            Error::NeedsRepair("control/bootstrap layout differs")
        ),
        "UNQUALIFIED bootstrap must refuse only the extra root entry: {error:?}"
    );
    eprintln!(
        "PASS B with unrelated registered descendant; exact bootstrap extra-layout refusal; no recursive registration-absence claim"
    );
}

#[test]
#[ignore = "real root registration at designated A final boundary; exact production query refusal"]
fn control_observation_a_late_root_registration() {
    let run = fixture::run("a-late-registration");
    let f = Fixture::new(&run, "late", FORMAT);
    let root = f.root.clone();
    let held = io::directory_observer(&root).unwrap();
    io::plain_directory(&held).unwrap();
    let before = io::id(&held).unwrap();
    assert_eq!(before, (f.allocated.volume, f.allocated.root_index));
    let reached = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = reached.clone();
    let registered = std::rc::Rc::new(std::cell::Cell::new(false));
    let completed = registered.clone();
    let result = fault::scoped(
        Box::new(move |point, _| {
            if point == fault::Point::BeforeFinalBoundary && !seen.replace(true) {
                register(&root, "late A root");
                completed.set(true);
            }
            Ok(())
        }),
        || observe_allocated_control_stage(&f.allocated),
    );
    assert!(reached.get(), "designated BeforeFinalBoundary not reached");
    assert!(
        registered.get(),
        "designated successful registration not completed"
    );
    let _fresh = registered_pair("late A root", &f.root, &held, before);
    let error = fixture::error(result);
    registered_refusal("late A root production final query", &error);
    assert!(matches!(&error, Error::Windows { .. }));
    fixture::unchanged_stage(&f, FORMAT);
    eprintln!(
        "PASS designated late A registration: exact production final query refusal {error:?}"
    );
}

#[test]
#[ignore = "connected ordinary-control metadata under current ALWAYS_FULL policy; non-discriminating counters only"]
fn control_observation_connected_metadata() {
    let run = fixture::run("connected-metadata-independent");
    let f = Fixture::new(&run, "connected", FORMAT);
    let expected = f.publish().identity().clone(); // drop sole owner before fresh expected-open
    let held = io::directory_observer(&f.root).unwrap();
    io::plain_directory(&held).unwrap();
    let before = io::id(&held).unwrap();
    register(&f.root, "connected ordinary control independent");
    let _fresh = registered_pair("connected root", &f.root, &held, before);
    connected(&f.root, || {
        let guard = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
        guard.revalidate().unwrap();
        let error = fixture::error(guard.revalidate_unregistered_root());
        let cause = b_cause(&f, &error, "revalidate unregistered root");
        registered_refusal("connected B production query", cause);
        assert!(matches!(cause, Error::Windows { .. }));
    });
    eprintln!(
        "PASS connected generic layout and exact B refusal; current ALWAYS_FULL/DISABLE_ON_DEMAND_ROOT counters are non-discriminating, no positive callback control, arbitrary-provider or allocated-stage A coverage claim"
    );
}
