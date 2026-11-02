use super::*;
use aidash_domain::{
	nodes::NodeName,
	scratch::{ScratchSettings, ScratchState},
};
use chrono::Utc;
use rstest::rstest;

use crate::apps::execution::test_database::{DatabaseFixture, database};

#[rstest]
#[tokio::test]
async fn space_and_settings_are_tenant_scoped_and_transactional(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let tenant_a = database.tenant("scratch-a").await;
	let tenant_b = database.tenant("scratch-b").await;
	let now = Utc::now();
	let mut space = ScratchSpace {
		id: Uuid::new_v4(),
		node: NodeName::derived("scratch-a"),
		name: "Project".into(),
		creator: "alice".into(),
		state: ScratchState::Active,
		settings: ScratchSettings::default(),
		members: BTreeSet::from(["bob".into()]),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = database.begin_in(&tenant_a).await;
	insert(&mut *tx, "scratch-a", &space).await.unwrap();
	tx.commit().await.unwrap();
	let mut neighbour = space.clone();
	neighbour.id = Uuid::new_v4();
	neighbour.node = NodeName::derived("scratch-b");
	let mut other = database.begin_in(&tenant_b).await;
	insert(&mut *other, "scratch-b", &neighbour).await.unwrap();
	other.commit().await.unwrap();
	let mut other = database.begin_in(&tenant_b).await;
	assert!(
		load(&mut *other, "scratch-a", space.id)
			.await
			.unwrap()
			.is_none()
	);
	other.commit().await.unwrap();
	let mut tx = database.begin_in(&tenant_a).await;
	let loaded = load(&mut *tx, "scratch-a", space.id)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(loaded.members, BTreeSet::from(["bob".into()]));
	space.members.insert("carol".into());
	space.settings.idle_expiry_days = None;
	space.clear();
	save(&mut *tx, "scratch-a", &space).await.unwrap();
	tx.rollback().await.unwrap();
	let mut tx = database.begin_in(&tenant_a).await;
	let loaded = load(&mut *tx, "scratch-a", space.id)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(loaded.members, BTreeSet::from(["bob".into()]));
	assert_eq!(loaded.settings.idle_expiry_days, Some(14));
	assert_eq!(loaded.state, ScratchState::Active);
	// Purge orders physical foreign keys, including the fixed conversation
	// binding, before deleting a Tenant's Scratch Spaces and Nodes.
	let references = crate::database::purge::references(&mut *tx).await.unwrap();
	let order = crate::database::purge::deletion_order(
		crate::database::classification::purged(),
		&references,
		crate::database::classification::UNLINKED,
	)
	.unwrap();
	let position = |name: &str| order.iter().position(|table| *table == name).unwrap();
	for table in [
		"scratch_spaces",
		"scratch_space_settings",
		"scratch_space_members",
	] {
		assert!(
			order.contains(&table),
			"{table} must be classified for Tenant purge"
		);
	}
	assert!(position("conversations") < position("scratch_spaces"));
	assert!(position("conversations") < position("nodes"));
	assert_eq!(
		ids_on_node(&mut *tx, "scratch-a", space.node.as_str())
			.await
			.unwrap(),
		vec![space.id]
	);
	delete(&mut *tx, "scratch-a", space.id).await.unwrap();
	tx.commit().await.unwrap();
	let mut tx = database.begin_in(&tenant_a).await;
	assert!(
		load(&mut *tx, "scratch-a", space.id)
			.await
			.unwrap()
			.is_none()
	);
	// Exercise the same classified table deletion used by Tenant purge.
	space.id = Uuid::new_v4();
	space.state = ScratchState::Active;
	insert(&mut *tx, "scratch-a", &space).await.unwrap();
	for table in order
		.iter()
		.filter(|table| table.starts_with("scratch_space"))
	{
		crate::database::purge::delete_tenant_rows(&mut *tx, table, "scratch-a")
			.await
			.unwrap();
	}
	assert!(
		load(&mut *tx, "scratch-a", space.id)
			.await
			.unwrap()
			.is_none()
	);
	tx.commit().await.unwrap();
	let mut other = database.begin_in(&tenant_b).await;
	assert!(
		load(&mut *other, "scratch-b", neighbour.id)
			.await
			.unwrap()
			.is_some()
	);
	other.commit().await.unwrap();
}

#[test]
fn remote_space_identity_is_stable_and_scoped_to_the_receiver_and_person() {
	let node = NodeName::derived("receiver");
	let workspace = Uuid::new_v4();
	let id = remote_space_id("tenant", &node, "aidash://home/node", workspace, "alice");
	assert_eq!(
		id,
		remote_space_id("tenant", &node, "aidash://home/node", workspace, "alice")
	);
	assert_ne!(
		id,
		remote_space_id("other", &node, "aidash://home/node", workspace, "alice")
	);
	assert_ne!(
		id,
		remote_space_id(
			"tenant",
			&NodeName::derived("other"),
			"aidash://home/node",
			workspace,
			"alice"
		)
	);
	assert_ne!(
		id,
		remote_space_id("tenant", &node, "aidash://home/node", workspace, "bob")
	);
	assert_ne!(
		id,
		remote_space_id("tenant", &node, "aidash://other/node", workspace, "alice")
	);
	assert_ne!(
		id,
		remote_space_id(
			"tenant",
			&node,
			"aidash://home/node",
			Uuid::new_v4(),
			"alice"
		)
	);
	assert_eq!(id.get_version_num(), 8);
}

#[test]
fn effective_operator_policy_applies_tenant_docker_and_host_restrictions() {
	let mut policy = crate::config::ScratchOperatorPolicy::default();
	let (limits, hosts) = effective_policy(&policy, "a");
	assert!(!limits.docker_allowed);
	assert!(matches!(hosts, HostAllowlist::Hosts(hosts) if hosts.contains("pypi.org")));
	policy.docker_tenants.insert("a".into());
	policy
		.tenant_hosts
		.insert("a".into(), vec!["example.com".into()]);
	let (limits, hosts) = effective_policy(&policy, "a");
	assert!(limits.docker_allowed);
	assert_eq!(
		hosts,
		HostAllowlist::Hosts(BTreeSet::from(["example.com".into()]))
	);
	assert!(!effective_policy(&policy, "b").0.docker_allowed);
	policy.ceilings.all_hosts_allowed = true;
	policy.tenant_hosts.insert("a".into(), vec!["*".into()]);
	assert_eq!(effective_policy(&policy, "a").1, HostAllowlist::All);
}
