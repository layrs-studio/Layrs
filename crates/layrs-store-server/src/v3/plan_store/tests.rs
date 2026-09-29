use super::*;

#[test]
fn capability_ids_are_deterministic_and_scoped() {
    let plan = layrs_sync::v3::PlanId::parse("plan:test").unwrap();
    assert_eq!(
        capability_id("upload", &plan, 3),
        capability_id("upload", &plan, 3)
    );
    assert_ne!(
        capability_id("upload", &plan, 3),
        capability_id("fetch", &plan, 3)
    );
}

#[test]
fn timestamp_is_rfc3339() {
    assert_eq!(timestamp(0).unwrap().as_str(), "1970-01-01T00:00:00Z");
}
