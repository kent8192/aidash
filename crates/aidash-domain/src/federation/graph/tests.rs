use super::*;
use rstest::rstest;
fn options() -> GraphOptions {
	GraphOptions {
		scope_workspace: None,
		depth: 1,
		mode: "mesh".into(),
		kinds: vec!["workspace".into(), "goal".into(), "task".into()],
		relations: vec!["contains".into(), "goal".into()],
		hours: 24,
		limit: 20,
		cursor: None,
		target_tenant: None,
	}
}
#[rstest]
#[case("depth")]
#[case("mode")]
#[case("limit_low")]
#[case("limit_high")]
#[case("hours")]
#[case("kinds")]
#[case("relations")]
#[case("cursor")]
#[case("kind")]
#[case("relation")]
#[case("tenant")]
fn projection_bounds_reject_invalid_input(#[case] field: &str) {
	let mut o = options();
	match field {
		"depth" => o.depth = 2,
		"mode" => o.mode = "unknown".into(),
		"limit_low" => o.limit = 1,
		"limit_high" => o.limit = 81,
		"hours" => o.hours = 721,
		"kinds" => o.kinds = vec!["task".into(); 13],
		"relations" => o.relations = vec!["contains".into(); 17],
		"cursor" => o.cursor = Some("x".repeat(4097)),
		"kind" => o.kinds = vec!["unknown".into()],
		"relation" => o.relations = vec!["unknown".into()],
		"tenant" => o.target_tenant = Some("bad tenant".into()),
		_ => panic!("invalid case"),
	};
	assert!(o.validate().is_err());
}
#[rstest]
#[case("topology", "workspace", false)]
#[case("topology", "agent", true)]
#[case("execution", "run", true)]
#[case("collaboration", "run", false)]
#[case("knowledge", "run", false)]
#[case("mesh", "run", true)]
fn modes_restrict_selected_kinds(#[case] mode: &str, #[case] kind: &str, #[case] expected: bool) {
	let mut o = options();
	o.mode = mode.into();
	o.kinds = vec![kind.into()];
	assert_eq!(kind_allowed(kind, &o), expected);
}
fn page() -> GraphPage {
	let node = "aidash://b";
	GraphPage {
		node_id: node.into(),
		generation: format!("sha256:{}", "a".repeat(64)),
		checked_at: Utc::now(),
		nodes: vec![GraphNode {
			id: resource_key(node, "workspace", Uuid::from_u128(1)),
			node_id: node.into(),
			kind: "workspace".into(),
			name: BTreeMap::new(),
			resource_id: Some(Uuid::from_u128(1).to_string()),
			version: None,
			workspace_id: Some(Uuid::from_u128(1)),
			status: None,
			goal_body: None,
			at: None,
		}],
		edges: vec![],
		activity: vec![],
		next_cursor: None,
	}
}
#[rstest]
#[case("source")]
#[case("node_source")]
#[case("key")]
#[case("duplicate")]
#[case("version")]
#[case("goal_body")]
#[case("generation")]
#[case("cursor")]
#[case("dangling_edge")]
#[case("relation")]
#[case("layer")]
#[case("dangling_activity")]
fn remote_projection_cannot_forge_scope_or_references(#[case] field: &str) {
	let mut p = page();
	let o = options();
	assert!(valid_remote_page(&p, "aidash://b", &o));
	match field {
		"source" => p.node_id = "aidash://c".into(),
		"node_source" => p.nodes[0].node_id = "aidash://c".into(),
		"key" => p.nodes[0].id = "forged".into(),
		"duplicate" => p.nodes.push(p.nodes[0].clone()),
		"version" => p.nodes[0].version = Some("1".into()),
		"goal_body" => p.nodes[0].goal_body = Some("hidden".into()),
		"generation" => p.generation = "sha256:bad".into(),
		"cursor" => p.next_cursor = Some("x".repeat(4097)),
		"dangling_activity" => p.activity.push(GraphActivity {
			kind: "workspace.updated".into(),
			at: p.checked_at,
			reference: "missing".into(),
		}),
		"dangling_edge" | "relation" | "layer" => p.edges.push(GraphEdge {
			source: p.nodes[0].id.clone(),
			target: if field == "dangling_edge" {
				"missing".into()
			} else {
				p.nodes[0].id.clone()
			},
			relation: if field == "relation" {
				"unknown".into()
			} else {
				"contains".into()
			},
			layer: if field == "layer" {
				"unknown".into()
			} else {
				"configuration".into()
			},
		}),
		_ => panic!("invalid case"),
	};
	assert!(!valid_remote_page(&p, "aidash://b", &o));
}
#[test]
fn scoped_keys_preserve_resource_and_definition_namespaces() {
	assert_ne!(
		resource_key("aidash://b", "agent", "one"),
		entity_key("aidash://b", "agent", "one", "1.0.0")
	);
	assert_eq!(
		entity_key("aidash://b", "agent", "one", "1.0.0"),
		json!(["entity", "aidash://b", "agent", "one", "1.0.0"]).to_string()
	);
}
