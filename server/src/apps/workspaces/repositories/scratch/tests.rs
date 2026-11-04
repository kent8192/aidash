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
		revision: 4,
		wipe_generation: 3,
		settings: ScratchSettings::default(),
		members: BTreeSet::from(["bob".into()]),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = database.begin_in(&tenant_a).await;
	insert(&mut *tx, "scratch-a", &space).await.unwrap();
	tx.commit().await.unwrap();
	// An unrelated transaction may inspect the space while a writer owns it.
	let mut writer = database.begin_in(&tenant_a).await;
	load(&mut *writer, "scratch-a", space.id).await.unwrap();
	let mut reader = database.begin_in(&tenant_a).await;
	let read = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		load_with_lock(&mut *reader, "scratch-a", space.id, false),
	)
	.await
	.expect("listing must not wait for a row lock")
	.unwrap();
	assert!(read.is_some());
	reader.rollback().await.unwrap();
	writer.rollback().await.unwrap();
	// A foreign-key check holds KEY SHARE. A stale touch must not wait for it,
	// and repeated fresh touches must leave the first timestamp intact.
	let mut key_holder = database.begin_in(&tenant_a).await;
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("id").eq(Expr::value(space.id)))
		.lock(LockType::KeyShare)
		.build(PostgresQueryBuilder);
	key_holder
		.fetch_optional(&sql, convert_values(values))
		.await
		.unwrap();
	let mut runner = database.begin_in(&tenant_a).await;
	let later = now + chrono::Duration::minutes(2);
	tokio::time::timeout(
		std::time::Duration::from_secs(5),
		touch(&mut *runner, "scratch-a", space.id, later),
	)
	.await
	.expect("a touch must not block a foreign-key check")
	.unwrap();
	touch(
		&mut *runner,
		"scratch-a",
		space.id,
		later + chrono::Duration::seconds(30),
	)
	.await
	.unwrap();
	let touched = load_with_lock(&mut *runner, "scratch-a", space.id, false)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(
		touched.last_used_at.timestamp_micros(),
		later.timestamp_micros()
	);
	runner.rollback().await.unwrap();
	key_holder.rollback().await.unwrap();
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
	assert_eq!(loaded.revision, 4);
	assert_eq!(loaded.wipe_generation, 3);
	space.members.insert("carol".into());
	space.settings.idle_expiry_days = None;
	space.tombstone();
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

#[rstest]
#[tokio::test]
async fn delegated_space_belongs_to_mapped_person_for_node_and_service_peers(
	#[future] database: DatabaseFixture,
	#[values("user", "service", "agent")] mapped_kind: &str,
) {
	use crate::apps::identity::services::{access::Access, identity::SubjectIdentity};
	use aidash_application::scratch::ScratchScope;
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		IntoValue as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	use serde_json::json;
	let database = database.await;
	// Peer Mappings may name a person or an exportable service credential.
	let tenant = &format!("scratch-delegated-{mapped_kind}");
	let mapped = match mapped_kind {
		"user" => "alice",
		"service" => "remote-alice",
		_ => "remote-agent",
	};
	let scope = database.tenant(tenant).await;
	let node = NodeName::derived(tenant);
	let credential = Uuid::new_v4();
	let tasks = [Uuid::new_v4(), Uuid::new_v4()];
	let workspace = Uuid::new_v4();
	let run_ids = [Uuid::new_v4(), Uuid::new_v4()];
	let peer_credentials = [Uuid::new_v4(), Uuid::new_v4()];
	let bundle: aidash_domain::policy::PolicyBundle = serde_json::from_value(json!({
        "tenant": tenant,
        "subjects": {mapped: {"kind":mapped_kind, "node": if mapped_kind == "user" { None } else { Some(node.as_str()) }}, "node-peer": {"kind":"service", "node":node.as_str()}, "service-peer": {"kind":"service", "node":node.as_str()}},
        "policies": [{"id":"node-use", "effect":"allow", "subjects":{"any":true}, "actions":["node.use", "scratch.create"], "resources":{"kinds":["node"]}}]
    })).unwrap();
	let mut tx = database.begin_in(&scope).await;
	let (sql, vals) = Query::insert()
		.into_table(Alias::new("authorization_bundles"))
		.columns(["tenant", "revision", "document"].map(Alias::new))
		.values_panic([
			tenant.into_value(),
			1_i64.into_value(),
			serde_json::to_value(&bundle).unwrap().into_value(),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(vals)).await.unwrap();
	let (sql, vals) = Query::insert()
		.into_table(Alias::new("authorization_credentials"))
		.columns(
			[
				"id",
				"tenant",
				"subject",
				"token_hash",
				"expires_at",
				"issued_by",
			]
			.map(Alias::new),
		)
		.values_panic([
			credential.into_value(),
			tenant.into_value(),
			mapped.into_value(),
			vec![7_u8; 32].into_value(),
			(Utc::now() + chrono::Duration::days(1)).into_value(),
			"alice".into_value(),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(vals)).await.unwrap();
	for (((id, task), peer), peer_credential) in run_ids
		.into_iter()
		.zip(tasks)
		.zip(["node-peer", "service-peer"])
		.zip(peer_credentials)
	{
		let (sql, vals) = Query::insert()
			.into_table(Alias::new("authorization_credentials"))
			.columns(
				[
					"id",
					"tenant",
					"subject",
					"token_hash",
					"expires_at",
					"issued_by",
				]
				.map(Alias::new),
			)
			.values_panic([
				peer_credential.into_value(),
				tenant.into_value(),
				peer.into_value(),
				vec![if peer == "node-peer" { 8_u8 } else { 9_u8 }; 32].into_value(),
				(Utc::now() + chrono::Duration::days(1)).into_value(),
				"alice".into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(vals)).await.unwrap();
		let (sql, vals) = Query::insert()
			.into_table(Alias::new("runs"))
			.columns(
				[
					"id",
					"tenant",
					"task_id",
					"workspace_id",
					"home_node",
					"node",
					"created_at",
					"agent_id",
					"agent_version",
				]
				.map(Alias::new),
			)
			.values_panic([
				id.into_value(),
				tenant.into_value(),
				task.into_value(),
				workspace.into_value(),
				"aidash://home".into_value(),
				node.as_str().into_value(),
				Utc::now().into_value(),
				"agent".into_value(),
				"1".into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(vals)).await.unwrap();
		let (sql, vals) = Query::insert()
			.into_table(Alias::new("authorization_remote_admissions"))
			.columns(
				[
					"id",
					"tenant",
					"node",
					"source_node",
					"grant_id",
					"task_id",
					"credential_id",
					"subject_chain",
					"description",
				]
				.map(Alias::new),
			)
			.values_panic([
				id.into_value(),
				tenant.into_value(),
				node.as_str().into_value(),
				"aidash://home".into_value(),
				Uuid::new_v4().into_value(),
				task.into_value(),
				credential.into_value(),
				reinhardt::query::Value::Array(
					reinhardt::query::ArrayType::String,
					Some(Box::new(vec![mapped.into_value(), peer.into_value()])),
				),
				json!({}).into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(vals)).await.unwrap();
	}
	tx.commit().await.unwrap();
	let pool = database.pool().await;
	let mut spaces = Vec::new();
	for (((id, task), peer), peer_credential) in run_ids
		.into_iter()
		.zip(tasks)
		.zip(["node-peer", "service-peer"])
		.zip(peer_credentials)
	{
		let raw: aidash_domain::RawRun = serde_json::from_value(json!({
			"id":id, "task_id":task, "workspace_id":workspace, "node":node,
			"home_node":"aidash://home", "agent_id":"agent", "agent_version":"1",
			"phase":"READY", "control":"ACTIVE", "step":0, "revision":0,
			"observed_input_seq":0, "ledger_worker_ready":true, "error":null,
			"lease_owner":null, "lease_until":null, "updated_at":Utc::now(),
			"context":aidash_domain::context::Context::default(),
			"pending":{"state_version":1,"data":{},"recovery":{"retry":null,"lease_recoveries":0}}
		}))
		.unwrap();
		let run = raw.decode().unwrap();
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: peer_credential,
			tenant: tenant.into(),
			subject: peer.into(),
		};
		let snapshot = aidash_application::authorization::Snapshot {
			revision: 1,
			bundle: bundle.clone(),
		};
		let mut access = Access::from_transaction(
			database.begin_in(&scope).await,
			&pool,
			"aidash://receiver",
			None,
			&identity,
			snapshot,
		);
		access.subjects = vec![mapped.into(), peer.into()];
		access.audit = false;
		let policy = crate::config::ScratchOperatorPolicy::default();
		let mut services = NativeScratchServices { policy: &policy };
		// A foreign-key check on this Node must not block reuse of a space.
		let mut held_node = if spaces.is_empty() {
			None
		} else {
			let mut tx = database.begin_in(&scope).await;
			let (sql, values) = Query::select()
				.column(Alias::new("name"))
				.from(Alias::new("nodes"))
				.and_where(Expr::col("tenant").eq(tenant.as_str()))
				.and_where(Expr::col("name").eq(node.as_str()))
				.lock(LockType::KeyShare)
				.build(PostgresQueryBuilder);
			tx.fetch_optional(&sql, convert_values(values))
				.await
				.unwrap()
				.unwrap();
			Some(tx)
		};
		let mut adapter = ScratchAdapter {
			access: &mut access,
			services: &mut services,
			person_override: None,
		};
		let (id, starter) =
			tokio::time::timeout(std::time::Duration::from_secs(5), adapter.run_binding(&run))
				.await
				.expect("reusing a delegated space must not wait for a Node foreign-key lock")
				.unwrap();
		if let Some(tx) = held_node.take() {
			tx.rollback().await.unwrap();
		}
		assert_eq!(starter, mapped);
		let space = adapter.load_for_update(id).await.unwrap();
		assert_eq!(space.creator, mapped);
		assert!(space.may_use(mapped));
		// Outside delegated allocation, users and services may own a space.
		adapter.person_override = None;
		assert_eq!(
			adapter.validate_owner(mapped).await.is_ok(),
			mapped_kind != "agent"
		);
		if mapped_kind == "service" {
			let dedicated = aidash_application::scratch::select_for_workspace(
				&mut adapter,
				&node,
				"Service workspace",
				None,
				Utc::now(),
			)
			.await
			.unwrap();
			assert_eq!(adapter.load(dedicated).await.unwrap().creator, peer);
		}
		spaces.push(id);
		drop(adapter);
		let inherited = crate::apps::identity::repositories::subnodes::subnode_scratch_binding(
			&mut access,
			&run.metadata(),
			&policy,
		)
		.await
		.unwrap();
		assert_eq!(inherited, (id, mapped.to_owned()));
		if spaces.len() == 2 {
			let mut adapter = ScratchAdapter {
				access: &mut access,
				services: &mut services,
				person_override: None,
			};
			assert_eq!(
				aidash_application::scratch::scratch_space_for_run(
					&mut adapter,
					&run,
					Utc::now() + chrono::Duration::days(16),
				)
				.await
				.unwrap(),
				id
			);
		}
		services.clear_contents(&mut access, id).await.unwrap();
		let reloaded = load(&mut **access.tx, tenant, id).await.unwrap().unwrap();
		assert_eq!(
			reloaded.wipe_generation as usize,
			spaces.len() + usize::from(spaces.len() == 2)
		);
		assert_eq!(reloaded.state, ScratchState::Active);
		access.finish(Ok(())).await.unwrap();
	}
	assert_eq!(spaces[0], spaces[1]);
}

#[rstest]
#[tokio::test]
async fn workspace_run_loses_scratch_authority_when_starter_is_removed(
	#[future] database: DatabaseFixture,
) {
	use crate::apps::identity::services::{access::Access, identity::SubjectIdentity};
	use aidash_application::scratch::scratch_space_for_run;
	use reinhardt::query::{IntoValue as _, QueryStatementBuilder as _};
	use serde_json::json;

	let database = database.await;
	let tenant = "workspace-scratch-revoke";
	let scope = database.tenant(tenant).await;
	let node = NodeName::derived(tenant);
	let workspace = Uuid::new_v4();
	let now = Utc::now();
	let mut space = ScratchSpace {
		id: Uuid::new_v4(),
		node: node.clone(),
		name: "Shared".into(),
		creator: "alice".into(),
		state: ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings: ScratchSettings::default(),
		members: BTreeSet::from(["bob".into()]),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = database.begin_in(&scope).await;
	insert(&mut *tx, tenant, &space).await.unwrap();
	let (sql, values) = Query::insert()
		.into_table(Alias::new("workspaces"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"title",
				"goal",
				"state",
				"revision",
				"scratch_space_id",
				"created_by",
			]
			.map(Alias::new),
		)
		.values_panic([
			workspace.into_value(),
			tenant.into_value(),
			node.as_str().into_value(),
			"Shared work".into_value(),
			"Work".into_value(),
			json!({}).into_value(),
			0_i64.into_value(),
			space.id.into_value(),
			"bob".into_value(),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await.unwrap();
	tx.commit().await.unwrap();
	let bundle: aidash_domain::policy::PolicyBundle = serde_json::from_value(json!({
        "tenant":tenant,"subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},
        "policies":[{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]
    })).unwrap();
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: Uuid::new_v4(),
		tenant: tenant.into(),
		subject: "alice".into(),
	};
	let snapshot = aidash_application::authorization::Snapshot {
		revision: 1,
		bundle,
	};
	let pool = database.pool().await;
	let mut access = Access::from_transaction(
		database.begin_in(&scope).await,
		&pool,
		"aidash://receiver",
		None,
		&identity,
		snapshot,
	);
	access.subjects = vec!["alice".into()];
	access.audit = false;
	let raw: aidash_domain::RawRun = serde_json::from_value(json!({
		"id":Uuid::new_v4(), "task_id":Uuid::new_v4(), "workspace_id":workspace, "node":node,
		"home_node":format!("aidash://receiver/{node}"), "agent_id":"agent", "agent_version":"1",
		"phase":"READY", "control":"ACTIVE", "step":0, "revision":0, "observed_input_seq":0,
		"ledger_worker_ready":true, "error":null, "lease_owner":null, "lease_until":null,
		"updated_at":now, "context":aidash_domain::context::Context::default(),
		"pending":{"state_version":1,"data":{},"recovery":{"retry":null,"lease_recoveries":0}}
	}))
	.unwrap();
	let run = raw.decode().unwrap();
	let policy = crate::config::ScratchOperatorPolicy::default();
	let mut services = NativeScratchServices { policy: &policy };
	let mut adapter = ScratchAdapter {
		access: &mut access,
		services: &mut services,
		person_override: None,
	};
	assert_eq!(
		scratch_space_for_run(&mut adapter, &run, now)
			.await
			.unwrap(),
		space.id
	);
	adapter
		.access
		.snapshot
		.bundle
		.subjects
		.get_mut("bob")
		.unwrap()
		.enabled = false;
	assert!(matches!(
		scratch_space_for_run(&mut adapter, &run, now).await,
		Err(aidash_application::Error::Forbidden)
	));
	adapter
		.access
		.snapshot
		.bundle
		.subjects
		.get_mut("bob")
		.unwrap()
		.enabled = true;
	space.members.clear();
	save(&mut **adapter.access.tx, tenant, &space)
		.await
		.unwrap();
	assert!(matches!(
		scratch_space_for_run(&mut adapter, &run, now).await,
		Err(aidash_application::Error::Forbidden)
	));
	drop(adapter);
	access.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn listing_filters_by_membership_and_loads_members(#[future] database: DatabaseFixture) {
	let database = database.await;
	let tenant = database.tenant("scratch-list").await;
	let now = Utc::now();
	let mut space = ScratchSpace {
		id: Uuid::new_v4(),
		node: NodeName::derived("scratch-list"),
		name: "Private".into(),
		creator: "alice".into(),
		state: ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings: ScratchSettings::default(),
		members: BTreeSet::new(),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = database.begin_in(&tenant).await;
	insert(&mut *tx, "scratch-list", &space).await.unwrap();
	let hidden = space.id;
	space.id = Uuid::new_v4();
	space.creator = "carol".into();
	space.members.insert("bob".into());
	insert(&mut *tx, "scratch-list", &space).await.unwrap();
	let result = list_visible(&mut *tx, "scratch-list", space.node.as_str(), "bob")
		.await
		.unwrap();
	assert_eq!(result.len(), 1);
	assert_ne!(result[0].id, hidden);
	assert_eq!(result[0].members, BTreeSet::from(["bob".into()]));
	assert_eq!(result[0].id, space.id);
	tx.rollback().await.unwrap();
}
