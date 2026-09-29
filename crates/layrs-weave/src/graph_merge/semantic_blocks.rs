//! The single-placement text subset of the normative Conflict schema.
//! Every fragment is a File object, including empty fragments. Original terms
//! remain immutable and anchors are byte offsets in those original files.
use super::*;

mod build;
mod resolve;
pub(super) use build::upgrade;
pub use resolve::ConflictBlockResolution;
pub(super) use resolve::resolve_blocks;

pub(super) fn reidentify(value: &mut Value) -> MergeResult<()> {
    value.as_object_mut().unwrap().remove("conflictId");
    value["conflictId"] = json!(digest("layrs-conflict-v1", value)?);
    Ok(())
}

fn block_id(conflict: &Value, block: &Value) -> MergeResult<String> {
    digest(
        "layrs-conflict-block-v1",
        &json!({
            "lensPin":conflict["lensPin"],
            "lensImplementationDigest":conflict["lensImplementationDigest"],
            "baseValues":block["baseValues"], "sideValues":block["sideValues"],
            "occurrenceAnchor":block["occurrenceAnchor"],
            "supportedMethods":block["supportedMethods"]
        }),
    )
    .map(|id| id.to_string())
}

fn array<'a>(value: &'a Value, key: &str) -> MergeResult<&'a Vec<Value>> {
    value[key]
        .as_array()
        .ok_or_else(|| invalid(format!("block {key} array")))
}

fn text(value: &Value) -> MergeResult<&str> {
    value
        .as_str()
        .ok_or_else(|| invalid("block string required"))
}

fn file_value(value: &Value, executable: bool) -> MergeResult<FileId> {
    let id = FileId::parse(text(&value["objectId"])?).map_err(invalid)?;
    if value != &json!({"kind":"file","objectId":id,"executable":executable}) {
        return Err(invalid("block requires exact file value and common mode"));
    }
    Ok(id)
}

fn originals(value: &Value) -> MergeResult<BTreeMap<String, (FileId, bool)>> {
    let bases = array(value, "bases")?;
    let sides = array(value, "sides")?;
    if bases.len() != 1
        || sides.len() != 2
        || value["kind"] != "content"
        || value["lensPin"].is_null()
    {
        return Err(GraphMergeError::Unsupported(
            "blocks require one real base, two file sides and a Lens pin".into(),
        ));
    }
    let mode = bases[0]["values"][0]["value"]["executable"]
        .as_bool()
        .ok_or_else(|| invalid("block original mode"))?;
    bases
        .iter()
        .chain(sides)
        .map(|term| {
            Ok((
                text(&term["termId"])?.to_owned(),
                (file_value(&term["values"][0]["value"], mode)?, mode),
            ))
        })
        .collect()
}

/// Validate exact shape and all intrinsic identities without loading file bytes.
/// Contextual reconstruction is performed separately after loading the closure.
pub(super) fn validate_shape(value: &Value, expected: &mut Value) -> MergeResult<()> {
    let original = originals(value)?;
    let mode = original.values().next().unwrap().1;
    let blocks = array(value, "blocks")?;
    let segments = array(value, "segments")?;
    if blocks.is_empty() || blocks.len() > 100_000 || segments.len() > 200_000 {
        return Err(GraphMergeError::Limit("canonical conflict blocks/segments"));
    }
    let mut ids = BTreeSet::new();
    let mut resolved = 0;
    let mut previous = None;
    for block in blocks {
        let id = block_id(value, block)?;
        if block["blockId"] != id || previous.as_ref().is_some_and(|p| p >= &id) {
            return Err(invalid("block identity/order"));
        }
        previous = Some(id.clone());
        ids.insert(id);
        let mut values = BTreeMap::new();
        for (role, key) in [("bases", "baseValues"), ("sides", "sideValues")] {
            let terms = array(value, role)?;
            let fragments = array(block, key)?;
            if fragments.len() != terms.len() {
                return Err(invalid("block term set"));
            }
            for (fragment, term) in fragments.iter().zip(terms) {
                if fragment["termId"] != term["termId"] {
                    return Err(invalid("block term order"));
                }
                file_value(&fragment["value"], mode)?;
                if fragment != &json!({"termId":term["termId"],"value":fragment["value"]}) {
                    return Err(invalid("block term fields"));
                }
                values.insert(
                    text(&fragment["termId"])?.to_owned(),
                    fragment["value"].clone(),
                );
            }
        }
        let anchor = &block["occurrenceAnchor"];
        let spans = array(anchor, "spans")?;
        if spans.len() != original.len() {
            return Err(invalid("block span term set"));
        }
        for (span, term) in spans.iter().zip(original.keys()) {
            let start = span["startByte"]
                .as_u64()
                .ok_or_else(|| invalid("block start"))?;
            let end = span["endByte"]
                .as_u64()
                .ok_or_else(|| invalid("block end"))?;
            if start > end || span != &json!({"termId":term,"startByte":start,"endByte":end}) {
                return Err(invalid("block span shape/order"));
            }
        }
        if anchor
            != &json!({"kind":"text","placementId":value["placements"][0]["placementId"],"spans":spans})
        {
            return Err(invalid("block anchor shape"));
        }
        let resolution = &block["resolution"];
        if !resolution.is_null() {
            resolved += 1;
            let sources = array(resolution, "sourceTermIds")?;
            let names = sources.iter().map(text).collect::<MergeResult<Vec<_>>>()?;
            if names.is_empty()
                || names.windows(2).any(|w| w[0] >= w[1])
                || names.iter().any(|id| !original.contains_key(*id))
            {
                return Err(invalid("block resolution source set"));
            }
            file_value(&resolution["result"], mode)?;
            match text(&resolution["method"])? {
                "select_term" if names.len() == 1 && resolution["result"] == values[names[0]] => {}
                "manual" => {}
                _ => return Err(invalid("unsupported block resolution method/result")),
            }
            if resolution
                != &json!({"method":resolution["method"],"sourceTermIds":sources,"result":resolution["result"]})
            {
                return Err(invalid("block resolution fields"));
            }
        }
        if block
            != &json!({"blockId":block["blockId"],"baseValues":block["baseValues"],
            "sideValues":block["sideValues"],"supportedMethods":["manual","select_term"],
            "occurrenceAnchor":anchor,"resolution":resolution})
        {
            return Err(invalid("block fields/methods"));
        }
    }
    let mut used = BTreeSet::new();
    for segment in segments {
        match text(&segment["kind"])? {
            "content" => {
                FileId::parse(text(&segment["objectId"])?).map_err(invalid)?;
                if segment != &json!({"kind":"content","objectId":segment["objectId"]}) {
                    return Err(invalid("content segment fields"));
                }
            }
            "block" => {
                let id = text(&segment["blockId"])?;
                if !ids.contains(id)
                    || !used.insert(id.to_owned())
                    || segment != &json!({"kind":"block","blockId":id})
                {
                    return Err(invalid("block segment identity/multiplicity"));
                }
            }
            _ => return Err(invalid("unknown conflict segment")),
        }
    }
    if used != ids || resolved == blocks.len() {
        return Err(invalid("unused or fully resolved blocks"));
    }
    expected["blocks"] = json!(blocks);
    expected["segments"] = json!(segments);
    expected["resolutionState"] = json!(if resolved == 0 {
        "unresolved"
    } else {
        "partial"
    });
    Ok(())
}

impl PlannedConflict {
    /// Additional immutable File closure needed for semantic display/resolution.
    pub fn payload_file_ids(&self) -> MergeResult<BTreeSet<FileId>> {
        let mut ids = BTreeSet::new();
        for block in array(&self.0, "blocks")? {
            for fragment in array(block, "baseValues")?
                .iter()
                .chain(array(block, "sideValues")?)
            {
                ids.insert(FileId::parse(text(&fragment["value"]["objectId"])?).map_err(invalid)?);
            }
            if !block["resolution"].is_null() {
                ids.insert(
                    FileId::parse(text(&block["resolution"]["result"]["objectId"])?)
                        .map_err(invalid)?,
                );
            }
        }
        for segment in array(&self.0, "segments")? {
            if segment["kind"] == "content" {
                ids.insert(FileId::parse(text(&segment["objectId"])?).map_err(invalid)?);
            }
        }
        Ok(ids)
    }

    /// Check that immutable fragments and spans reconstruct all three originals
    /// exactly. Never use a search for an ambiguous repeated substring.
    pub fn validate_payload(&self, files: &BTreeMap<FileId, Vec<u8>>) -> MergeResult<()> {
        if array(&self.0, "blocks")?.is_empty() {
            return Ok(());
        }
        let original = originals(&self.0)?;
        let blocks: BTreeMap<_, _> = array(&self.0, "blocks")?
            .iter()
            .map(|b| (b["blockId"].as_str().unwrap(), b))
            .collect();
        for (term, (id, _)) in original {
            let bytes = file_bytes(files, &id)?;
            let mut cursor = 0usize;
            for segment in array(&self.0, "segments")? {
                let fragment = if segment["kind"] == "content" {
                    FileId::parse(text(&segment["objectId"])?).map_err(invalid)?
                } else {
                    let block = blocks[text(&segment["blockId"])?];
                    let span = array(&block["occurrenceAnchor"], "spans")?
                        .iter()
                        .find(|s| s["termId"] == term)
                        .unwrap();
                    if span["startByte"].as_u64() != Some(cursor as u64) {
                        return Err(invalid("block anchor position"));
                    }
                    let fragment = array(block, "baseValues")?
                        .iter()
                        .chain(array(block, "sideValues")?)
                        .find(|v| v["termId"] == term)
                        .unwrap();
                    let id =
                        FileId::parse(text(&fragment["value"]["objectId"])?).map_err(invalid)?;
                    if span["endByte"].as_u64()
                        != Some(cursor as u64 + file_bytes(files, &id)?.len() as u64)
                    {
                        return Err(invalid("block anchor length"));
                    }
                    id
                };
                let fragment = file_bytes(files, &fragment)?;
                let end = cursor
                    .checked_add(fragment.len())
                    .ok_or(GraphMergeError::Limit("block offset"))?;
                if bytes.get(cursor..end) != Some(fragment) {
                    return Err(invalid("block original reconstruction"));
                }
                cursor = end;
            }
            if cursor != bytes.len() {
                return Err(invalid("truncated conflict original"));
            }
        }
        for id in self.payload_file_ids()? {
            file_bytes(files, &id)?;
        }
        Ok(())
    }
}

fn file_bytes<'a>(files: &'a BTreeMap<FileId, Vec<u8>>, id: &FileId) -> MergeResult<&'a [u8]> {
    let bytes = files
        .get(id)
        .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
    if FileId::from_bytes(bytes) != *id || std::str::from_utf8(bytes).is_err() || bytes.contains(&0)
    {
        return Err(invalid("block file hash/text"));
    }
    Ok(bytes)
}
