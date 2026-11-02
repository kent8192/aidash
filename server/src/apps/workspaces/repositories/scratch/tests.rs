use super::*;
use aidash_domain::{
	nodes::NodeName,
	scratch::{ScratchSettings, ScratchState},
};
use chrono::Utc;
use rstest::rstest;

#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
use native_database::{DatabaseFixture, database};

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
	save(&mut *tx, "scratch-a", &space).await.unwrap();
	tx.rollback().await.unwrap();
	let mut tx = database.begin_in(&tenant_a).await;
	let loaded = load(&mut *tx, "scratch-a", space.id)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(loaded.members, BTreeSet::from(["bob".into()]));
	assert_eq!(loaded.settings.idle_expiry_days, Some(14));
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
	tx.commit().await.unwrap();
}
