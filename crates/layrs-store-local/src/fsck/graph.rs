use std::collections::HashMap;

const MAX_OBJECT_GRAPH_ISSUES: usize = 100;

pub(super) fn cycle_edges(edges: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut indices = HashMap::<String, usize>::new();
    let mut labels = Vec::<String>::new();
    for (parent, child) in &edges {
        for label in [parent, child] {
            if !indices.contains_key(label) {
                let index = labels.len();
                labels.push(label.clone());
                indices.insert(label.clone(), index);
            }
        }
    }

    let mut adjacency = vec![Vec::new(); labels.len()];
    for (parent, child) in edges {
        adjacency[indices[&parent]].push(indices[&child]);
    }

    let mut state = vec![0_u8; labels.len()];
    let mut cycles = Vec::new();
    for start in 0..labels.len() {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0_usize)];
        while let Some((node, next_child)) = stack.last_mut() {
            if *next_child == adjacency[*node].len() {
                state[*node] = 2;
                stack.pop();
                continue;
            }
            let child = adjacency[*node][*next_child];
            *next_child += 1;
            match state[child] {
                0 => {
                    state[child] = 1;
                    stack.push((child, 0));
                }
                1 if cycles.len() < MAX_OBJECT_GRAPH_ISSUES => {
                    cycles.push((labels[*node].clone(), labels[child].clone()));
                }
                _ => {}
            }
        }
    }
    cycles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_dag_is_linear_in_vertices_and_edges_without_false_cycles() {
        let mut edges = Vec::new();
        for parent in 0..128 {
            for child in (parent + 1)..128 {
                edges.push((format!("node-{parent}"), format!("node-{child}")));
            }
        }
        assert!(cycle_edges(edges).is_empty());
    }

    #[test]
    fn deep_cycle_is_iterative_and_issue_count_is_bounded() {
        let depth = 20_000;
        let mut edges = (0..depth)
            .map(|node| (format!("node-{node}"), format!("node-{}", node + 1)))
            .collect::<Vec<_>>();
        edges.push((format!("node-{depth}"), "node-0".to_string()));
        let cycles = cycle_edges(edges);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].1, "node-0");
    }

    #[test]
    fn reported_cycle_edges_are_bounded() {
        let mut edges = Vec::new();
        for cycle in 0..200 {
            edges.push((format!("a-{cycle}"), format!("b-{cycle}")));
            edges.push((format!("b-{cycle}"), format!("a-{cycle}")));
        }
        assert_eq!(cycle_edges(edges).len(), MAX_OBJECT_GRAPH_ISSUES);
    }
}
