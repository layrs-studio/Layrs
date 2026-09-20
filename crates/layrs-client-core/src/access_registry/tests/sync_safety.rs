use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn read_http_request(stream: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("read mock request");
        assert!(read > 0, "mock request closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("request headers utf8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("read mock request body");
        assert!(read > 0, "mock request closed before body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    let body = if content_length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_length])
            .expect("request body json")
    };
    (headers, body)
}

fn write_http_json(stream: &mut TcpStream, body: &Value) {
    let body = serde_json::to_vec(body).expect("response json");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write mock response headers");
    stream.write_all(&body).expect("write mock response");
}

fn accept_with_timeout(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for mock request"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept mock request: {error}"),
        }
    }
}

fn clear_pending_publish_for_test(store: &mut LocalStoreSession, layrs_dir: &Path, layer_id: &str) {
    let directory = pending_publish_dir(layrs_dir, layer_id);
    store
        .remove_listed::<PendingPublishFile>(&directory, ".json")
        .expect("clear pending publish documents");
}

fn spawn_publish_server(
    acknowledge_all: bool,
) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    listener
        .set_nonblocking(true)
        .expect("set mock listener nonblocking");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock addr"));
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut prepare_stream = accept_with_timeout(&listener);
        let (prepare_headers, _) = read_http_request(&mut prepare_stream);
        assert!(prepare_headers.contains("/chunks/prepare"));
        write_http_json(&mut prepare_stream, &serde_json::json!({ "items": [] }));
        drop(prepare_stream);

        let mut publish_stream = accept_with_timeout(&listener);
        let (publish_headers, publish_body) = read_http_request(&mut publish_stream);
        assert!(publish_headers.contains("/sync/publish"));
        let requested_steps = publish_body["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        sender.send(publish_body).expect("send captured publish");
        let acknowledged = if acknowledge_all {
            requested_steps
                .iter()
                .map(|step| serde_json::json!({ "stepId": step["stepId"] }))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        write_http_json(
            &mut publish_stream,
            &serde_json::json!({ "serverCursor": "cursor_test", "steps": acknowledged }),
        );
    });
    (endpoint, receiver, worker)
}

fn linked_space_with_two_net_zero_pending_steps(
    prefix: &str,
    endpoint: &str,
) -> (PathBuf, PathBuf, LocalSpaceHandle, Vec<String>) {
    let root = unique_test_dir(prefix);
    let config_root = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config_root);
    env::set_var("LAYRS_DESKTOP_TEST_TOKEN", "desktop_test_token");
    fs::write(space.join("note.txt"), "base\n").unwrap();
    let created = create_local_space(
        "sync-safety".to_string(),
        space.display().to_string(),
        Some("layer_main_sync_safety".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.state = LOCAL_SPACE_STATE_LINKED.to_string();
    handle.meta.workspace_id = "workspace_sync_safety".to_string();
    handle.meta.space_id = "space_sync_safety".to_string();
    handle.meta.server_space_id = Some(handle.meta.space_id.clone());
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    clear_pending_publish_for_test(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
    );

    fs::write(space.join("note.txt"), "changed\n").unwrap();
    let changed =
        capture_working_state(&mut handle.store, &space, &handle.active.layer_id).unwrap();
    let changed_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &changed,
    )
    .unwrap();
    let changed_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &changed_id,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &changed_step).unwrap();

    fs::write(space.join("note.txt"), "base\n").unwrap();
    let reverted =
        capture_working_state(&mut handle.store, &space, &handle.active.layer_id).unwrap();
    let reverted_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &reverted,
    )
    .unwrap();
    let reverted_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &reverted_id,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &reverted_step).unwrap();

    let mut config = DesktopConfig::load_or_create().unwrap();
    config.server_endpoint = endpoint.to_string();
    config.save().unwrap();
    (root, space, handle, vec![changed_id, reverted_id])
}

#[test]
fn net_zero_publish_sends_all_pending_steps_in_order_and_clears_only_after_ack() {
    if !isolated_legacy_test(
        "net_zero_publish_sends_all_pending_steps_in_order_and_clears_only_after_ack",
    ) {
        return;
    }
    let (endpoint, request, server) = spawn_publish_server(true);
    let (root, space, handle, step_ids) =
        linked_space_with_two_net_zero_pending_steps("net-zero-publish", &endpoint);

    let local_id = handle.meta.local_space_id.clone();
    drop(handle);
    let result = publish_local_space(local_id.clone()).unwrap();
    let handle = open_local_space_handle(&local_id).unwrap();
    let body = request.recv().expect("captured publish body");
    let sent_ids = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["stepId"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(sent_ids, step_ids);
    assert_eq!(result.status, "published");
    assert!(
        read_pending_publish_files(&handle.store, &handle.layrs_dir, &handle.active.layer_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(space.join("note.txt")).unwrap(), b"base\n");
    server.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_with_incomplete_step_ack_keeps_every_pending_step_and_user_file() {
    if !isolated_legacy_test(
        "publish_with_incomplete_step_ack_keeps_every_pending_step_and_user_file",
    ) {
        return;
    }
    let (endpoint, request, server) = spawn_publish_server(false);
    let (root, space, handle, step_ids) =
        linked_space_with_two_net_zero_pending_steps("incomplete-publish-ack", &endpoint);

    let local_id = handle.meta.local_space_id.clone();
    drop(handle);
    let error = publish_local_space(local_id.clone()).unwrap_err();
    let handle = open_local_space_handle(&local_id).unwrap();
    let _ = request.recv().expect("captured publish body");
    assert!(error.contains("acknowledgement mismatch"), "{error}");
    let remaining =
        read_pending_publish_files(&handle.store, &handle.layrs_dir, &handle.active.layer_id)
            .unwrap()
            .into_iter()
            .map(|pending| pending.step_id)
            .collect::<Vec<_>>();
    assert_eq!(remaining, step_ids);
    assert_eq!(fs::read(space.join("note.txt")).unwrap(), b"base\n");
    server.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_rejects_unexpected_step_acknowledgements() {
    if !isolated_legacy_test("publish_rejects_unexpected_step_acknowledgements") {
        return;
    }
    let requested = vec!["step-1".to_string()];
    let response = serde_json::json!({
        "steps": [
            {"stepId": "step-1"},
            {"stepId": "step-unexpected"}
        ]
    });

    let error = acknowledged_publish_step_ids_exact(&response, &requested).unwrap_err();
    assert!(error.contains("unexpected: step-unexpected"), "{error}");
}

#[test]
fn receive_rejects_a_tree_owned_by_another_layer_before_mutating_local_state() {
    if !isolated_legacy_test(
        "receive_rejects_a_tree_owned_by_another_layer_before_mutating_local_state",
    ) {
        return;
    }
    let root = unique_test_dir("receive-wrong-layer-tree");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "local\n").unwrap();
    let created = create_local_space(
        "wrong-layer-tree".to_string(),
        space.display().to_string(),
        Some("layer_expected".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    let pending_before =
        read_pending_publish_files(&handle.store, &handle.layrs_dir, "layer_expected").unwrap();
    let wrong_tree_id = blake3_id(b"wrong-layer-tree");
    let response = ReceiveLocalSpaceResponse {
        workspace_id: "workspace_1".to_string(),
        space_id: "space_1".to_string(),
        layer_id: "layer_expected".to_string(),
        protocol: Some(SYNC_PROTOCOL_V2.to_string()),
        root_tree_id: Some(wrong_tree_id.clone()),
        cursor: None,
        layers: vec![ReceivedLayer {
            id: "layer_expected".to_string(),
            workspace_id: Some("workspace_1".to_string()),
            space_id: Some("space_1".to_string()),
            name: "Main".to_string(),
            parent_layer_id: None,
            access: Some("open".to_string()),
        }],
        access_registries: Vec::new(),
        content_objects: Some(ReceivedContentObjects {
            chunks: Vec::new(),
            file_objects: Vec::new(),
            tree_objects: vec![ReceivedTreeObject {
                tree_id: wrong_tree_id,
                layer_id: Some("layer_other".to_string()),
                entries: Vec::new(),
            }],
        }),
        timeline: Vec::new(),
        steps: Vec::new(),
    };

    let error = apply_receive_response(&mut handle, response, true).unwrap_err();
    assert!(error.contains("exact tree"), "{error}");
    assert_eq!(fs::read(space.join("note.txt")).unwrap(), b"local\n");
    assert_eq!(
        read_pending_publish_files(&handle.store, &handle.layrs_dir, "layer_expected")
            .unwrap()
            .len(),
        pending_before.len()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn received_metadata_preserves_an_existing_disconnected_lineage() {
    if !isolated_legacy_test("received_metadata_preserves_an_existing_disconnected_lineage") {
        return;
    }
    let existing = LocalLayerMetadata {
        layer_id: "feature".to_string(),
        display_name: "Feature".to_string(),
        parent_layer_id: Some("main".to_string()),
        lineage_status: "unlinked".to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
    };
    let received = ReceivedLayer {
        id: "feature".to_string(),
        workspace_id: None,
        space_id: None,
        name: "Feature remote".to_string(),
        parent_layer_id: Some("main".to_string()),
        access: Some("open".to_string()),
    };

    let merged = received_layer_metadata(&received, Some(&existing));
    assert_eq!(merged.lineage_status, "unlinked");
    assert_eq!(merged.parent_layer_id.as_deref(), Some("main"));
}

#[test]
fn conflicted_sync_refuses_continue_when_a_new_pending_step_appears() {
    if !isolated_legacy_test("conflicted_sync_refuses_continue_when_a_new_pending_step_appears") {
        return;
    }
    let root = unique_test_dir("sync-weave-pending-generation");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "base\n").unwrap();
    let created = create_local_space(
        "sync-generation".to_string(),
        space.display().to_string(),
        Some("layer_server".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    clear_pending_publish_for_test(&mut handle.store, &handle.layrs_dir, "layer_server");
    let base_state = read_layer_index(&handle.store, &handle.layrs_dir, "layer_server").unwrap();

    fs::write(space.join("note.txt"), "studio\n").unwrap();
    let studio_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    fs::write(space.join("note.txt"), "local\n").unwrap();
    let local_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    let local_step_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_state,
    )
    .unwrap();
    let local_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_step_id,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &local_step).unwrap();
    let pending = pending_publish_steps(&handle.store, &handle.layrs_dir, "layer_server").unwrap();
    let sync_state_path = handle.layrs_dir.join("sync-state.json");
    let result = weave_local_state_over_received_sync(
        &mut handle,
        Some(&base_state),
        &local_state,
        &studio_state,
        &pending,
        &sync_state_path,
        "sync",
    )
    .unwrap()
    .expect("sync conflict");
    assert_eq!(result.status, "conflicted");
    let marker_bytes = fs::read(space.join("note.txt")).unwrap();

    let mut late_step = local_step.clone();
    late_step.step_id = "step_created_after_conflict".to_string();
    late_step.parent_step_id = Some(local_step_id);
    late_step.timeline_position =
        Some(next_timeline_position(&handle.store, &handle.layrs_dir, "layer_server").unwrap());
    write_json(
        &mut handle.store,
        &layer_dir(&handle.layrs_dir, "layer_server")
            .join("steps")
            .join("step_created_after_conflict.json"),
        &late_step,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &late_step).unwrap();

    drop(handle);
    let conflicts = weave_conflicts(created.local_space.local_space_id.clone()).unwrap();
    for conflict in conflicts {
        for block in conflict.blocks {
            resolve_weave_conflict(
                created.local_space.local_space_id.clone(),
                conflict.path.clone(),
                format!("block:{}:incoming", block.block_id),
                None,
                None,
            )
            .unwrap();
        }
    }
    let error = continue_weave(created.local_space.local_space_id.clone()).unwrap_err();
    assert!(error.contains("pending Step set changed"), "{error}");
    assert_eq!(fs::read(space.join("note.txt")).unwrap(), marker_bytes);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert_eq!(
        read_pending_publish_files(&handle.store, &handle.layrs_dir, "layer_server")
            .unwrap()
            .len(),
        2
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_manifest_keeps_objects_from_an_added_then_deleted_pending_path() {
    if !isolated_legacy_test(
        "publish_manifest_keeps_objects_from_an_added_then_deleted_pending_path",
    ) {
        return;
    }
    let root = unique_test_dir("publish-historical-closure");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    let created = create_local_space(
        "historical-closure".to_string(),
        space.display().to_string(),
        Some("layer_history".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let base = read_layer_index(&handle.store, &handle.layrs_dir, "layer_history").unwrap();
    fs::write(space.join("transient.txt"), "must remain reachable\n").unwrap();
    let added_state = capture_working_state(&mut handle.store, &space, "layer_history").unwrap();
    let added_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        "layer_history",
        &added_state,
    )
    .unwrap();
    let added_step =
        read_step_file(&handle.store, &handle.layrs_dir, "layer_history", &added_id).unwrap();
    fs::remove_file(space.join("transient.txt")).unwrap();
    let deleted_state = capture_working_state(&mut handle.store, &space, "layer_history").unwrap();
    let deleted_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        "layer_history",
        &deleted_state,
    )
    .unwrap();
    let deleted_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        "layer_history",
        &deleted_id,
    )
    .unwrap();
    let desktop_config = DesktopConfig::load_or_create().unwrap();

    let request = build_publish_v2_request(
        &handle,
        &desktop_config,
        "layer_history",
        base.root_tree_id,
        &deleted_state,
        vec!["transient.txt".to_string()],
        vec!["transient.txt".to_string()],
        &[added_step.clone(), deleted_step],
    )
    .unwrap();
    assert!(request
        .store_objects
        .tree_objects
        .iter()
        .any(|tree| tree.tree_id == added_step.root_tree_id.clone().unwrap()));
    assert!(request
        .store_objects
        .file_objects
        .iter()
        .any(|object| object.file_object_id == added_state.files[0].hash));
    assert!(!request.store_objects.chunks.is_empty());
    let _ = fs::remove_dir_all(root);
}
