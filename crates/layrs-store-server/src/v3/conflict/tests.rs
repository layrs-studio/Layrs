use super::*;
use serde_json::{Value, json};

fn id(digit: char) -> String {
    format!("blake3:{}", digit.to_string().repeat(64))
}

fn nested_conflict() -> Value {
    let placement = id('1');
    let base = id('2');
    let left = id('3');
    let right = id('4');
    let step_a = id('5');
    let step_b = id('6');
    let step_c = id('7');
    let block = id('8');
    json!({
        "schemaVersion": "layrs.scm.conflict.v1",
        "conflictId": id('9'),
        "kind": "content",
        "placements": [{
            "placementId": placement,
            "path": "src/lib.rs",
            "role": "primary"
        }],
        "bases": [{
            "termId": base,
            "termKind": "base",
            "provenance": {
                "kind": "virtual",
                "sourceStepIds": [step_a, step_b],
                "algorithm": {"id": "recursive.base", "version": "1"},
                "result": {"kind": "tree", "objectId": id('a')}
            },
            "values": [{
                "placementId": placement,
                "value": {"kind": "file", "objectId": id('b'), "executable": false}
            }]
        }],
        "sides": [
            {
                "termId": left,
                "termKind": "side",
                "provenance": {"kind": "step", "stepId": step_b},
                "values": [{
                    "placementId": placement,
                    "value": {"kind": "file", "objectId": id('c'), "executable": true}
                }]
            },
            {
                "termId": right,
                "termKind": "side",
                "provenance": {"kind": "step", "stepId": step_c},
                "values": [{
                    "placementId": placement,
                    "value": {"kind": "tree", "objectId": id('d'), "executable": null}
                }]
            }
        ],
        "lensPin": {
            "lensId": "text.rust",
            "version": "1.2.3-beta+build",
            "digest": format!("sha256:{}", "1".repeat(64))
        },
        "lensImplementationDigest": format!("sha256:{}", "2".repeat(64)),
        "renderedObjectId": null,
        "resolutionState": "unresolved",
        "placementResolutions": [],
        "blocks": [{
            "blockId": block,
            "baseValues": [{
                "termId": base,
                "value": {"kind": "file", "objectId": id('b'), "executable": false}
            }],
            "sideValues": [
                {
                    "termId": left,
                    "value": {"kind": "file", "objectId": id('c'), "executable": true}
                },
                {
                    "termId": right,
                    "value": {"kind": "tree", "objectId": id('d'), "executable": null}
                }
            ],
            "supportedMethods": ["manual", "select_term"],
            "occurrenceAnchor": {
                "kind": "text",
                "placementId": placement,
                "spans": [
                    {"termId": left, "startByte": 0, "endByte": 1},
                    {"termId": right, "startByte": 1, "endByte": 2}
                ]
            },
            "resolution": null
        }],
        "segments": [
            {"kind": "content", "objectId": id('e')},
            {"kind": "block", "blockId": block}
        ],
        "originStepIds": [step_a, step_b, step_c]
    })
}

#[test]
fn nested_normative_variants_are_typed_and_validated() {
    let value = nested_conflict();
    let conflict: ConflictObject = serde_json::from_value(value.clone()).unwrap();
    conflict.validate().unwrap();
    let round_trip = serde_json::to_value(conflict).unwrap();
    assert_eq!(round_trip, value);
}

#[test]
fn closure_contains_external_edges_not_embedded_record_ids() {
    let edges =
        conflict_object_references(&serde_jcs::to_vec(&nested_conflict()).unwrap()).unwrap();
    let ids = edges
        .iter()
        .map(|(id, _)| id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let expected = ['5', '6', '7', 'a', 'b', 'c', 'd', 'e']
        .into_iter()
        .map(id)
        .collect();
    assert_eq!(
        ids, expected,
        "terms, blocks and placements must remain embedded"
    );
    assert!(edges.contains(&(
        ObjectId::parse(id('a')).unwrap(),
        Some(layrs_sync::v3::ObjectKind::Tree)
    )));
    assert!(edges.contains(&(ObjectId::parse(id('b')).unwrap(), None)));
}

#[test]
fn present_nullable_fields_are_required() {
    let mut value = nested_conflict();
    value["sides"][0]["values"][0]["value"]
        .as_object_mut()
        .unwrap()
        .remove("executable");
    assert!(serde_json::from_value::<ConflictObject>(value).is_err());

    for field in ["lensPin", "lensImplementationDigest", "renderedObjectId"] {
        let mut value = nested_conflict();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<ConflictObject>(value).is_err(),
            "{field}"
        );
    }

    let mut value = nested_conflict();
    value["blocks"][0]
        .as_object_mut()
        .unwrap()
        .remove("resolution");
    assert!(serde_json::from_value::<ConflictObject>(value).is_err());
}
