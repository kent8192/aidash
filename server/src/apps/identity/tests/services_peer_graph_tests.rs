//! Unit tests for services::peer::graph.
use super::*;

#[test]
fn rejects_multihop_graph_requests() {
	let options = GraphOptions {
		scope_workspace: None,
		depth: 2,
		mode: "mesh".into(),
		kinds: vec!["agent".into()],
		relations: vec!["hosts".into()],
		hours: 24,
		limit: 20,
		cursor: None,
		target_tenant: None,
	};
	assert!(options.validate().is_err());
}

#[test]
fn source_rejects_cross_node_and_dangling_projection() {
	let input = GraphExpandInput {
		node_id: "aidash://b".into(),
		options: GraphOptions {
			scope_workspace: None,
			depth: 1,
			mode: "mesh".into(),
			kinds: vec!["agent".into()],
			relations: vec!["hosts".into()],
			hours: 24,
			limit: 20,
			cursor: None,
			target_tenant: None,
		},
	};
	let mut page = GraphPage {
		node_id: "aidash://b".into(),
		generation: format!("sha256:{}", "a".repeat(64)),
		checked_at: Utc::now(),
		nodes: vec![GraphNode {
			id: entity_key("aidash://b", "agent", "one", "1.0.0"),
			node_id: "aidash://b".into(),
			kind: "agent".into(),
			name: BTreeMap::from([("en".into(), "One".into())]),
			resource_id: Some("one".into()),
			version: Some("1.0.0".into()),
			workspace_id: None,
			status: None,
			goal_body: None,
			at: None,
		}],
		edges: vec![],
		activity: vec![],
		next_cursor: None,
	};
	assert!(valid_remote_page(&page, &input));
	page.nodes[0].id = entity_key("aidash://c", "agent", "one", "1.0.0");
	assert!(!valid_remote_page(&page, &input));
	page.nodes[0].id = entity_key("aidash://b", "agent", "one", "1.0.0");
	page.edges.push(GraphEdge {
		source: page.nodes[0].id.clone(),
		target: entity_key("aidash://b", "agent", "hidden", "1.0.0"),
		relation: "hosts".into(),
		layer: "federation".into(),
	});
	assert!(!valid_remote_page(&page, &input));
}
