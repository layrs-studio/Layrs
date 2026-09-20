//! Graph, admission, current reference and retained-root verification.
use super::*;
impl Context<'_> {
    pub(super) fn check_graphs(&mut self) -> Result<(), FsckError> {
        let step_nodes = query_node_spaces(self.connection, "SELECT step_id, '' FROM steps")?;
        let step_edges = query_edges(
            self.connection,
            "SELECT step_id, parent_step_id FROM step_parents",
        )?;
        if !self.consume_cpu(step_nodes.len().saturating_add(step_edges.len())) {
            return Ok(());
        }
        let step_adjacency = adjacency(&step_edges);
        self.check_parent_graph(
            &step_nodes,
            &step_edges,
            FsckFindingCode::StepParentMissing,
            FsckFindingCode::StepCycle,
            "Step",
        );
        let canonical_steps: Vec<_> = self
            .objects
            .iter()
            .filter_map(|(id, object)| match object {
                CanonicalObject::Step(step) => Some((id.clone(), step.clone())),
                _ => None,
            })
            .collect();
        for (id, step) in canonical_steps {
            let projected = self
                .connection
                .query_row(
                    "SELECT root_tree_id, parent_count FROM steps WHERE step_id=?1",
                    [&id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .optional()?;
            let parents = query_strings(
                self.connection,
                "SELECT parent_step_id FROM step_parents WHERE step_id=?1 ORDER BY ordinal",
                &id,
            )?;
            let expected_parents: Vec<_> = step
                .parent_step_ids()
                .iter()
                .map(ToString::to_string)
                .collect();
            if projected.as_ref().is_none_or(|(root, count)| {
                root != step.root_tree_id().as_str() || *count != expected_parents.len() as u64
            }) || parents != expected_parents
            {
                self.projection_divergence(
                    "steps/step_parents",
                    &id,
                    "Step projection differs from canonical CAS",
                );
            }
        }

        let operation_nodes = query_node_spaces(
            self.connection,
            "SELECT operation_id, space_id FROM operations",
        )?;
        let operation_edges = query_edges(
            self.connection,
            "SELECT operation_id, parent_operation_id FROM operation_parents",
        )?;
        if !self.consume_cpu(operation_nodes.len().saturating_add(operation_edges.len())) {
            return Ok(());
        }
        self.check_parent_graph(
            &operation_nodes,
            &operation_edges,
            FsckFindingCode::OperationParentMissing,
            FsckFindingCode::OperationCycle,
            "Operation",
        );
        for (child, parent) in &operation_edges {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            if operation_nodes.get(child) != operation_nodes.get(parent) {
                self.error(
                    FsckFindingCode::CrossSpaceReference,
                    Some(child.clone()),
                    format!("Operation parent {parent} belongs to another Space"),
                );
            }
        }
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT o.operation_id, o.operation_kind, COUNT(p.parent_operation_id) \
                 FROM operations o LEFT JOIN operation_parents p ON p.operation_id=o.operation_id \
                 GROUP BY o.operation_id, o.operation_kind",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, kind, parent_count) in rows {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            let invalid = (kind == "initialize") != (parent_count == 0)
                || (kind == "integrate_operations" && parent_count < 2);
            if invalid {
                self.error(
                    FsckFindingCode::OperationHistoryInvalid,
                    Some(id),
                    format!("{kind} has {parent_count} parents"),
                );
            }
        }
        self.check_observation_reachability(&step_adjacency)?;
        Ok(())
    }

    pub(super) fn check_observation_reachability(
        &mut self,
        step_adjacency: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<(), FsckError> {
        let observations = query_triples(
            self.connection,
            "SELECT observation_id, operation_id, step_id FROM step_observations
             WHERE operation_id IS NOT NULL ORDER BY observation_id",
        )?;
        for (observation, operation, step) in observations {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            let heads = query_strings(
                self.connection,
                "SELECT h.step_id FROM operations o
                 JOIN repository_view_layer_heads h
                   ON h.repository_view_id=o.resulting_view_id
                 WHERE o.operation_id=?1 ORDER BY h.layer_id, h.ordinal",
                &operation,
            )?;
            let mut reachable = false;
            for head in heads {
                if !self.consume_cpu(1) {
                    return Ok(());
                }
                if head == step {
                    reachable = true;
                    break;
                }
                match graph_reaches_bounded(
                    step_adjacency,
                    &head,
                    &step,
                    &mut self.cpu_budget_remaining,
                ) {
                    Some(true) => {
                        reachable = true;
                        break;
                    }
                    Some(false) => {}
                    None => {
                        self.exhaust_cpu();
                        return Ok(());
                    }
                }
            }
            if !reachable {
                self.error(
                    FsckFindingCode::OperationHistoryInvalid,
                    Some(observation),
                    format!(
                        "StepObservation Operation {operation} does not reach observed Step {step}"
                    ),
                );
            }
        }
        Ok(())
    }

    pub(super) fn check_parent_graph(
        &mut self,
        nodes: &BTreeMap<String, String>,
        edges: &[(String, String)],
        missing_code: FsckFindingCode,
        cycle_code: FsckFindingCode,
        label: &str,
    ) {
        for (child, parent) in edges {
            if !nodes.contains_key(child) || !nodes.contains_key(parent) {
                self.error(
                    missing_code,
                    Some(child.clone()),
                    format!("{label} parent {parent} is missing"),
                );
            }
        }
        for member in cyclic_nodes(nodes.keys().cloned(), edges) {
            self.error(
                cycle_code,
                Some(member),
                format!("{label} parent graph contains a cycle"),
            );
        }
    }

    pub(super) fn check_admissions(&mut self) -> Result<(), FsckError> {
        let missing = query_triples(
            self.connection,
            "SELECT a.space_id, a.object_id, e.target_object_id \
             FROM space_object_admissions a JOIN object_edges e ON e.source_object_id=a.object_id \
             WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions child \
                               WHERE child.space_id=a.space_id AND child.object_id=e.target_object_id)",
        )?;
        for (space, source, target) in missing {
            self.finding(
                FsckFindingCode::AdmissionClosureBroken,
                FsckSeverity::Error,
                None,
                Some(source),
                Some(space),
                format!("transitive object {target} is not admitted to the same Space"),
            );
        }
        let missing_roots = query_triples(
            self.connection,
            "SELECT a.space_id, a.object_id, s.root_tree_id \
             FROM space_object_admissions a JOIN steps s ON s.step_id=a.object_id \
             WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions child \
                               WHERE child.space_id=a.space_id AND child.object_id=s.root_tree_id)",
        )?;
        for (space, step, tree) in missing_roots {
            self.finding(
                FsckFindingCode::AdmissionClosureBroken,
                FsckSeverity::Error,
                None,
                Some(step),
                Some(space),
                format!("Step root Tree {tree} is not admitted to the same Space"),
            );
        }
        Ok(())
    }

    pub(super) fn check_views_and_spaces(&mut self) -> Result<(), FsckError> {
        let step_edges = query_edges(
            self.connection,
            "SELECT step_id, parent_step_id FROM step_parents",
        )?;
        if !self.consume_cpu(step_edges.len()) {
            return Ok(());
        }
        let step_adjacency = adjacency(&step_edges);
        let heads = {
            let mut statement = self.connection.prepare(
                "SELECT repository_view_id, layer_id, step_id FROM repository_view_layer_heads \
                 ORDER BY repository_view_id, layer_id, ordinal",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if !self.consume_cpu(heads.len()) {
            return Ok(());
        }
        let mut by_view_layer: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for (view, layer, step) in heads {
            by_view_layer.entry((view, layer)).or_default().push(step);
        }
        for ((view, layer), values) in by_view_layer {
            for (index, left) in values.iter().enumerate() {
                for right in values.iter().skip(index + 1) {
                    if !self.consume_cpu(1) {
                        return Ok(());
                    }
                    let left_reaches = graph_reaches_bounded(
                        &step_adjacency,
                        left,
                        right,
                        &mut self.cpu_budget_remaining,
                    );
                    let right_reaches = graph_reaches_bounded(
                        &step_adjacency,
                        right,
                        left,
                        &mut self.cpu_budget_remaining,
                    );
                    let (Some(left_reaches), Some(right_reaches)) = (left_reaches, right_reaches)
                    else {
                        self.exhaust_cpu();
                        return Ok(());
                    };
                    if left_reaches || right_reaches {
                        self.error(
                            FsckFindingCode::ViewHeadsNotAntichain,
                            Some(view.clone()),
                            format!("Layer {layer} heads {left} and {right} are comparable"),
                        );
                    }
                }
            }
        }

        for (entity, reason) in cross_space_violations(self.connection)? {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            self.error(FsckFindingCode::CrossSpaceReference, Some(entity), reason);
        }
        Ok(())
    }

    pub(super) fn check_current_references(&mut self) -> Result<(), FsckError> {
        let invalid_views = query_pairs(
            self.connection,
            "SELECT c.space_id, c.repository_view_id FROM current_repository_views c \
             LEFT JOIN repository_views v ON v.repository_view_id=c.repository_view_id \
             WHERE c.generation < 1 OR v.repository_view_id IS NULL OR v.space_id<>c.space_id",
        )?;
        for (space, view) in invalid_views {
            self.error(
                FsckFindingCode::CurrentReferenceInvalid,
                Some(space),
                format!("current RepositoryView {view} is missing or cross-Space"),
            );
        }
        let invalid_heads = query_pairs(
            self.connection,
            "SELECT h.space_id, h.operation_id FROM operation_heads h \
             LEFT JOIN operations o ON o.operation_id=h.operation_id \
             WHERE o.operation_id IS NULL OR o.space_id<>h.space_id",
        )?;
        for (space, operation) in invalid_heads {
            self.error(
                FsckFindingCode::CurrentReferenceInvalid,
                Some(space),
                format!("Operation head {operation} is missing or cross-Space"),
            );
        }
        let missing_transactions = query_pairs(
            self.connection,
            "SELECT c.space_id, c.updated_by_transaction_id FROM current_repository_views c
             WHERE NOT EXISTS (SELECT 1 FROM applied_transactions t WHERE t.transaction_id=c.updated_by_transaction_id)
             UNION ALL
             SELECT h.space_id, h.updated_by_transaction_id FROM operation_heads h
             WHERE NOT EXISTS (SELECT 1 FROM applied_transactions t WHERE t.transaction_id=h.updated_by_transaction_id)",
        )?;
        for (space, transaction) in missing_transactions {
            self.error(
                FsckFindingCode::CurrentReferenceInvalid,
                Some(space),
                format!("reference generation cites missing transaction {transaction}"),
            );
        }
        let head_gaps = query_strings_no_param(
            self.connection,
            "SELECT c.space_id FROM current_repository_views c
             WHERE NOT EXISTS (SELECT 1 FROM operation_heads h WHERE h.space_id=c.space_id)
             UNION
             SELECT h.space_id FROM operation_heads h
             WHERE NOT EXISTS (SELECT 1 FROM current_repository_views c WHERE c.space_id=h.space_id)",
        )?;
        for space in head_gaps {
            self.error(
                FsckFindingCode::CurrentReferenceInvalid,
                Some(space),
                "current View and Operation heads must coexist",
            );
        }
        let operation_edges = query_edges(
            self.connection,
            "SELECT operation_id, parent_operation_id FROM operation_parents",
        )?;
        if !self.consume_cpu(operation_edges.len()) {
            return Ok(());
        }
        let operation_adjacency = adjacency(&operation_edges);
        let heads = query_pairs(
            self.connection,
            "SELECT space_id, operation_id FROM operation_heads ORDER BY space_id, ordinal",
        )?;
        if !self.consume_cpu(heads.len()) {
            return Ok(());
        }
        let mut by_space: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (space, operation) in heads {
            by_space.entry(space).or_default().push(operation);
        }
        for (space, operations) in by_space {
            for (index, left) in operations.iter().enumerate() {
                for right in operations.iter().skip(index + 1) {
                    if !self.consume_cpu(1) {
                        return Ok(());
                    }
                    let left_reaches = graph_reaches_bounded(
                        &operation_adjacency,
                        left,
                        right,
                        &mut self.cpu_budget_remaining,
                    );
                    let right_reaches = graph_reaches_bounded(
                        &operation_adjacency,
                        right,
                        left,
                        &mut self.cpu_budget_remaining,
                    );
                    let (Some(left_reaches), Some(right_reaches)) = (left_reaches, right_reaches)
                    else {
                        self.exhaust_cpu();
                        return Ok(());
                    };
                    if left_reaches || right_reaches {
                        self.error(
                            FsckFindingCode::CurrentReferenceInvalid,
                            Some(space.clone()),
                            format!("Operation heads {left} and {right} are not an antichain"),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn compute_gc_reachability(&mut self) -> Result<(), FsckError> {
        let actual_rows = {
            let mut statement = self.connection.prepare(
                "SELECT root_kind, root_id, object_id FROM gc_roots ORDER BY root_kind, root_id, object_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<BTreeSet<_>>>()?
        };
        let expected_rows: BTreeSet<_> = query_gc_expected(self.connection)?.into_iter().collect();
        for (kind, root, object) in actual_rows.symmetric_difference(&expected_rows) {
            self.finding(
                FsckFindingCode::GcRootInvalid,
                FsckSeverity::Error,
                None,
                Some(object.clone()),
                Some(root.clone()),
                format!("GC root projection mismatch for {kind}"),
            );
        }
        // `gc_roots` is the sole reachability inventory. Corrupt substitute
        // rows must not accidentally retain arbitrary objects, and missing
        // expected rows must remain visible as missing roots. Therefore only
        // rows that exactly match the independently derived contract seed the
        // traversal.
        let roots: Vec<_> = actual_rows
            .intersection(&expected_rows)
            .map(|(_, _, object)| object.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let edges = query_edges(
            self.connection,
            "SELECT source_object_id, target_object_id FROM object_edges",
        )?;
        let adjacency = adjacency(&edges);
        if !self.consume_cpu(roots.len().saturating_add(edges.len())) {
            return Ok(());
        }
        let known: BTreeSet<_> = self.object_ids.keys().cloned().collect();
        let mut manifests_by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (object_id, object) in &self.objects {
            if let CanonicalObject::FileManifest(manifest) = object {
                manifests_by_file
                    .entry(manifest.file_id().to_string())
                    .or_default()
                    .push(object_id.clone());
            }
        }
        let mut reachable = BTreeSet::new();
        let mut queue = VecDeque::new();
        for root in &roots {
            if known.contains(root) {
                queue.push_back(root.clone());
            } else {
                self.finding(
                    FsckFindingCode::GcRootInvalid,
                    FsckSeverity::Error,
                    None,
                    Some(root.clone()),
                    None,
                    "GC root references a missing object",
                );
            }
        }
        while let Some(object) = queue.pop_front() {
            if !self.consume_cpu(1) {
                return Ok(());
            }
            if !reachable.insert(object.clone()) {
                continue;
            }
            if let Some(targets) = adjacency.get(&object) {
                if !self.consume_cpu(targets.len()) {
                    return Ok(());
                }
                queue.extend(targets.iter().cloned());
            }
            // A Tree names logical FileIds, not one preferred physical
            // manifest. Every verified manifest for that FileId is therefore
            // part of the GC closure; their canonical manifest_chunk edges
            // retain every referenced Chunk.
            let tree_files: Vec<_> = match self.objects.get(&object) {
                Some(CanonicalObject::Tree(tree)) => tree
                    .entries()
                    .iter()
                    .filter_map(|entry| entry.target().file_id().map(ToString::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            for file_id in tree_files {
                if !self.consume_cpu(1) {
                    return Ok(());
                }
                if let Some(manifests) = manifests_by_file.get(&file_id) {
                    if !self.consume_cpu(manifests.len()) {
                        return Ok(());
                    }
                    queue.extend(manifests.iter().cloned());
                }
            }
            let manifest_chunks: Vec<_> = match self.objects.get(&object) {
                Some(CanonicalObject::FileManifest(manifest)) => manifest
                    .chunks()
                    .iter()
                    .map(|entry| entry.chunk_id().to_string())
                    .collect(),
                _ => Vec::new(),
            };
            if !manifest_chunks.is_empty() {
                if !self.consume_cpu(manifest_chunks.len()) {
                    return Ok(());
                }
                queue.extend(manifest_chunks);
            }
        }
        let unreachable: Vec<_> = known.difference(&reachable).cloned().collect();
        if unreachable.len() > self.max_findings {
            self.report.truncated_count = self
                .report
                .truncated_count
                .saturating_add((unreachable.len() - self.max_findings) as u64);
        }
        self.report.gc_reachability = GcReachability {
            roots: roots.len() as u64,
            reachable_objects: reachable.len() as u64,
            unreachable_objects: unreachable.into_iter().take(self.max_findings).collect(),
        };
        Ok(())
    }
}
