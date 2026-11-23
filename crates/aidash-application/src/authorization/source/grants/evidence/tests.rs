use super::*;
use chrono::Utc;
use rstest::rstest;
use uuid::Uuid;

#[rstest]
#[case::new_grant("grant_id")]
#[case::renewed_grant("grant_expiry")]
#[case::new_bound_task_revision("bound_task")]
#[case::new_live_task_revision("live_task")]
#[case::new_agent_version("agent_version")]
#[case::expired("expiry")]
#[tokio::test(start_paused = true)]
async fn receiver_evidence_is_reused_only_within_the_same_grant_revision(#[case] change: &str) {
	let node =
		aidash_domain::nodes::NodeAddress::derived("aidash://receiver", "receiver").to_string();
	let home = "aidash://home";
	let mut grant = Grant {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		task_revision: 7,
		thread_id: Uuid::from_u128(3),
		node_id: node.clone(),
		tenant: "acme".into(),
		credential_id: Uuid::from_u128(4),
		root_subject: "requester".into(),
		subject_chain: vec!["requester".into()],
		inspection: json!({
			"node_id":node, "authority_digest":"authority",
			"agent":crate::test_support::agent("agent"), "definitions":[],
			"binding_snapshot":crate::test_support::snapshot("aidash://receiver", "agent")
		}),
		expires_at: Utc::now() + chrono::Duration::minutes(5),
		revoked: false,
		semantic: json!({"mode":"disabled"}),
	};
	let cache = Arc::<GrantPeerCache>::default();
	let revision = Revision::new(&grant, 7, home);
	let inspection: Inspection = serde_json::from_value(grant.inspection.clone()).unwrap();
	cache.remember_inspection(revision.clone(), Observed::new(inspection.clone()));
	let checks = DependencyChecks::new(cache.clone(), revision.clone());
	let route = crate::test_support::foreign_route(&node);
	let input = Input {
		reference: Reference::Admission {
			node_id: node,
			home_node: home.into(),
			grant_id: grant.id,
			admission_id: Uuid::from_u128(6),
		},
	};
	assert!(matches!(
		checks.check(&route, &input, TTL).await,
		Check::NotAsked
	));
	let (miss_route, miss_input) = checks.take_missing().unwrap();
	assert_eq!(miss_route, route);
	assert_eq!(miss_input.reference, input.reference);
	assert!(checks.remember(
		route.clone(),
		input.clone(),
		Check::Confirmed(Checked {
			visible: true,
			pending: vec![],
		})
	));
	tokio::time::advance(TTL).await;
	assert_eq!(cache.inspection(&revision).unwrap().value, inspection);
	assert!(matches!(
		checks.check(&route, &input, TTL).await,
		Check::Confirmed(Checked { visible: true, .. })
	));

	let mut live_revision = 7;
	match change {
		"grant_id" => grant.id = Uuid::from_u128(99),
		"grant_expiry" => grant.expires_at += chrono::Duration::seconds(1),
		"bound_task" => grant.task_revision += 1,
		"live_task" => live_revision += 1,
		"agent_version" => grant.inspection["agent"]["version"] = json!("2.0.0"),
		"expiry" => tokio::time::advance(Duration::from_millis(1)).await,
		_ => unreachable!(),
	}
	let changed = Revision::new(&grant, live_revision, home);
	assert!(cache.inspection(&changed).is_none());
	let checks = DependencyChecks::new(cache, changed);
	assert!(matches!(
		checks.check(&route, &input, TTL).await,
		Check::NotAsked
	));
}
