#[derive(Deserialize)]
#[serde(transparent)]
struct PublishStoreObjectsBody(PublishCanonicalStoreObjectsBody);

impl PublishStoreObjectsBody {
    fn into_flat(self) -> Result<Vec<PublishStoreObjectBody>, ApiError> {
        self.0.into_flat()
    }
}

#[derive(Default, Deserialize)]
struct PublishCanonicalStoreObjectsBody {
    #[serde(default)]
    chunks: Vec<PublishChunkObjectBody>,
    #[serde(default)]
    file_objects: Vec<PublishFileObjectBody>,
    #[serde(default, rename = "fileObjects")]
    file_objects_camel: Vec<PublishFileObjectBody>,
    #[serde(default)]
    tree_objects: Vec<PublishTreeObjectBody>,
    #[serde(default, rename = "treeObjects")]
    tree_objects_camel: Vec<PublishTreeObjectBody>,
    #[serde(default)]
    tombstones: Vec<PublishTombstoneObjectBody>,
    #[serde(default)]
    deleted_paths: Vec<String>,
    #[serde(default, rename = "deletedPaths")]
    deleted_paths_camel: Vec<String>,
}

impl PublishCanonicalStoreObjectsBody {
    fn into_flat(mut self) -> Result<Vec<PublishStoreObjectBody>, ApiError> {
        self.file_objects.extend(self.file_objects_camel);
        self.tree_objects.extend(self.tree_objects_camel);
        self.deleted_paths.extend(self.deleted_paths_camel);

        let mut chunks_by_id = HashMap::new();
        for chunk in self.chunks {
            let chunk_id = chunk
                .chunk_id
                .clone()
                .or_else(|| chunk.chunk_id_camel.clone())
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.chunks[].chunkId is required")
                })?;
            chunks_by_id.insert(chunk_id, chunk);
        }

        let mut objects = Vec::new();
        for tree in self.tree_objects {
            let tree_id = tree
                .tree_id
                .or(tree.tree_id_camel)
                .or(tree.object_id)
                .or(tree.object_id_camel)
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.treeObjects[].treeId is required")
                })?;
            objects.push(PublishStoreObjectBody {
                object_type: Some("tree".to_string()),
                object_type_camel: None,
                object_id: Some(tree_id),
                object_id_camel: None,
                path: None,
                hash: None,
                digest: None,
                size: Some(tree.entries.len() as u64),
                size_bytes: None,
                size_bytes_camel: None,
                media_type: None,
                media_type_camel: None,
                chunks: Vec::new(),
                entries: tree.entries,
            });
        }

        for file in self.file_objects {
            let file_object_id = file
                .file_object_id
                .or(file.file_object_id_camel)
                .or(file.object_id)
                .or(file.object_id_camel)
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.fileObjects[].fileObjectId is required")
                })?;
            let mut chunks = Vec::new();
            for chunk_ref in file.chunks {
                let chunk_id = chunk_ref
                    .chunk_id
                    .or(chunk_ref.chunk_id_camel)
                    .ok_or_else(|| ApiError::bad_request("file object chunkId is required"))?;
                let source = chunks_by_id.get(&chunk_id);
                chunks.push(PublishStoreObjectChunkBody {
                    chunk_id: Some(chunk_id),
                    chunk_id_camel: None,
                    digest: source.and_then(|chunk| chunk.digest.clone()),
                    hash: source.and_then(|chunk| chunk.hash.clone()),
                    size: chunk_ref
                        .size
                        .or_else(|| source.and_then(|chunk| chunk.size)),
                    size_bytes: chunk_ref
                        .size_bytes
                        .or(chunk_ref.size_bytes_camel)
                        .or(chunk_ref.raw_size)
                        .or_else(|| {
                            source.and_then(|chunk| {
                                chunk
                                    .size_bytes
                                    .or(chunk.size_bytes_camel)
                                    .or(chunk.raw_size)
                            })
                        }),
                    size_bytes_camel: None,
                    raw_size: None,
                    stored_size: chunk_ref
                        .stored_size
                        .or_else(|| source.and_then(|chunk| chunk.stored_size)),
                    compression: chunk_ref
                        .compression
                        .or_else(|| source.and_then(|chunk| chunk.compression.clone())),
                    byte_offset: chunk_ref.byte_offset.or(chunk_ref.byte_offset_camel),
                    byte_offset_camel: None,
                });
            }
            objects.push(PublishStoreObjectBody {
                object_type: Some("file".to_string()),
                object_type_camel: None,
                object_id: Some(file_object_id),
                object_id_camel: None,
                path: file.path,
                hash: file.digest.or(file.hash),
                digest: None,
                size: file.size,
                size_bytes: file.size_bytes.or(file.size_bytes_camel),
                size_bytes_camel: None,
                media_type: file.media_type.or(file.media_type_camel),
                media_type_camel: None,
                chunks,
                entries: Vec::new(),
            });
        }

        for tombstone in self.tombstones {
            if let Some(path) = tombstone.path {
                self.deleted_paths.push(path);
            }
        }
        for path in self.deleted_paths {
            objects.push(PublishStoreObjectBody {
                object_type: Some("tombstone".to_string()),
                object_type_camel: None,
                object_id: None,
                object_id_camel: None,
                path: Some(path),
                hash: None,
                digest: None,
                size: None,
                size_bytes: None,
                size_bytes_camel: None,
                media_type: None,
                media_type_camel: None,
                chunks: Vec::new(),
                entries: Vec::new(),
            });
        }

        Ok(objects)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishChunkObjectBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default, rename = "rawSize")]
    raw_size: Option<i64>,
    #[serde(default, rename = "storedSize")]
    stored_size: Option<i64>,
    #[serde(default)]
    compression: Option<String>,
}

#[derive(Deserialize)]
struct PublishFileObjectBody {
    #[serde(default)]
    file_object_id: Option<String>,
    #[serde(default, rename = "fileObjectId")]
    file_object_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, rename = "mediaType")]
    media_type_camel: Option<String>,
    #[serde(default)]
    chunks: Vec<PublishChunkRefBody>,
}

#[derive(Deserialize)]
struct PublishChunkRefBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default, rename = "rawSize")]
    raw_size: Option<i64>,
    #[serde(default, rename = "storedSize")]
    stored_size: Option<i64>,
    #[serde(default)]
    compression: Option<String>,
    #[serde(default)]
    byte_offset: Option<i64>,
    #[serde(default, rename = "byteOffset")]
    byte_offset_camel: Option<i64>,
}

#[derive(Deserialize)]
struct PublishTreeObjectBody {
    #[serde(default)]
    tree_id: Option<String>,
    #[serde(default, rename = "treeId")]
    tree_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    entries: Vec<PublishTreeEntryBody>,
}

#[derive(Deserialize)]
struct PublishTreeEntryBody {
    path: String,
    #[serde(default)]
    file_object_id: Option<String>,
    #[serde(default, rename = "fileObjectId")]
    file_object_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
}

#[derive(Deserialize)]
struct PublishTombstoneObjectBody {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct PublishStoreObjectBody {
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default, rename = "objectType")]
    object_type_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, rename = "mediaType")]
    media_type_camel: Option<String>,
    #[serde(default)]
    chunks: Vec<PublishStoreObjectChunkBody>,
    #[serde(default)]
    entries: Vec<PublishTreeEntryBody>,
}

#[derive(Deserialize)]
struct PublishStoreObjectChunkBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default, rename = "rawSize")]
    raw_size: Option<i64>,
    #[serde(default, rename = "storedSize")]
    stored_size: Option<i64>,
    #[serde(default)]
    compression: Option<String>,
    #[serde(default)]
    byte_offset: Option<i64>,
    #[serde(default, rename = "byteOffset")]
    byte_offset_camel: Option<i64>,
}
