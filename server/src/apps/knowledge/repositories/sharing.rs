//! Tenant-controlled reach of built-in cross-Run memory.
use crate::{Error, Result, database::native};
use aidash_domain::memory::SharingScope;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<(SharingScope, i64)> {
	native::query(
		&Query::select()
			.column(Alias::new("name"))
			.from(Alias::new("tenants"))
			.and_where(Expr::col("name").eq(tenant))
			.lock(LockType::KeyShare)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	let row = native::query(
		&Query::select()
			.columns(["scope", "revision"].map(Alias::new))
			.from(Alias::new("memory_tenant_scope"))
			.and_where(Expr::col("tenant").eq(tenant))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(tx)
	.await?;
	match row {
		Some(row) => Ok((
			serde_json::from_value(serde_json::Value::String(row.try_get("scope")?))?,
			row.try_get("revision")?,
		)),
		None => Ok((SharingScope::Workspace, 0)),
	}
}

pub(crate) async fn set(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	scope: SharingScope,
	expected: i64,
) -> Result<i64> {
	native::query(
		&Query::select()
			.column(Alias::new("name"))
			.from(Alias::new("tenants"))
			.and_where(Expr::col("name").eq(tenant))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	let (_, current) = get(tx, tenant).await?;
	if current != expected || current < 0 || current == i64::MAX {
		return Err(Error::Conflict("memory scope revision changed".into()));
	}
	let revision = current + 1;
	let value = serde_json::to_value(scope)?
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid memory scope".into()))?
		.to_owned();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_tenant_scope"))
			.columns(["tenant", "scope", "revision", "updated_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(tenant))
					.expr(Expr::value(value))
					.expr(Expr::value(revision))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.update_columns(["scope", "revision", "updated_at"].map(Alias::new))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(revision)
}

/// A cross-Workspace Unit belongs to an Agent only if every provenance leaf
/// names one of its Runs. Derived observations inherit all source leaves.
pub(crate) async fn agent_unit(
	lease: &mut super::access::Lease<'_>,
	unit: &aidash_domain::memory::Unit,
	agent: &str,
	bound: usize,
) -> Result<bool> {
	use aidash_domain::memory::Evidence;
	use std::collections::{BTreeSet, VecDeque};
	let mut pending: VecDeque<_> = unit.content.evidence.iter().cloned().collect();
	let mut seen = BTreeSet::new();
	let mut visits = 0usize;
	let mut has_run = false;
	while let Some(source) = pending.pop_front() {
		visits += 1;
		if visits > bound {
			return Err(Error::Invalid(
				"memory sharing provenance exceeds its bound".into(),
			));
		}
		match source {
			Evidence::JournalRange { run_id, .. } => {
				has_run = true;
				let owner: Option<String> = native::query_scalar(
					&Query::select()
						.column(Alias::new("agent_id"))
						.from(Alias::new("runs"))
						.and_where(Expr::col("id").eq(Expr::value(run_id)))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **lease.tx())
				.await?;
				if !SharingScope::TenantAgent.permits(true, false, owner.as_deref() == Some(agent))
				{
					return Ok(false);
				}
			}
			Evidence::Unit { bank, id, revision } => {
				if bank != unit.bank {
					return Ok(false);
				}
				if !seen.insert((id, revision)) {
					continue;
				}
				let Some(source) = super::units::load(lease, id, false).await? else {
					return Ok(false);
				};
				if source.bank != bank || source.revision != revision || !source.visible() {
					return Ok(false);
				}
				pending.extend(source.content.evidence);
			}
			_ => return Ok(false),
		}
	}
	Ok(has_run)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::apps::execution::test_database::{DatabaseFixture, database};
	use crate::apps::knowledge::repositories::fixture::{bounds, setup};
	use aidash_application::tenancy::TEST_FIXTURE;
	use aidash_domain::memory::*;
	use rstest::rstest;
	use uuid::Uuid;

	#[rstest]
	#[tokio::test]
	async fn tenant_scope_requires_the_same_agent_on_every_provenance_leaf(
		#[future] database: DatabaseFixture,
		bounds: Bounds,
	) {
		let database = database.await;
		let (store, _, workspace) = setup(&database, bounds).await;
		let mut runs = Vec::new();
		for agent in ["a", "other-agent"] {
			let task = store
				.create_task(
					&TEST_FIXTURE,
					workspace,
					&aidash_domain::NewTask {
						title: "source".into(),
						description: "source".into(),
						requirements: serde_json::json!({}),
						dependencies: vec![],
						parent_id: None,
					},
					"deployment_administrator",
					None,
				)
				.await
				.unwrap();
			let run = Uuid::now_v7();
			let pending = aidash_domain::run_state::encode(
				&aidash_domain::run_state::RunState::Completed(
					aidash_domain::run_state::TerminalState {},
				),
				&aidash_domain::run_state::RecoveryState::default(),
			)
			.unwrap();
			let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
			native::query(
				&Query::insert()
					.into_table(Alias::new("runs"))
					.columns(
						[
							"id",
							"task_id",
							"workspace_id",
							"home_node",
							"node",
							"agent_id",
							"agent_version",
							"phase",
							"pending",
							"revision",
							"tenant",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::value(run))
							.expr(Expr::value(task.id))
							.expr(Expr::value(workspace))
							.expr(Expr::value(
								aidash_domain::nodes::NodeAddress::derived(
									store.deployment.as_str(),
									"acme",
								)
								.to_string(),
							))
							.expr(Expr::value(
								aidash_domain::nodes::NodeName::derived("acme").to_string(),
							))
							.expr(Expr::value(agent))
							.expr(Expr::value("1.0.0"))
							.expr(Expr::value("COMPLETED"))
							.expr(Expr::value(pending))
							.expr(Expr::value(1_i64))
							.expr(Expr::value("acme"))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await
			.unwrap();
			tx.commit().await.unwrap();
			runs.push(run);
		}
		let bank = Bank {
			home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
				.to_string(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		};
		let mut unit = Unit {
			id: Uuid::now_v7(),
			bank,
			revision: 1,
			content: Content {
				mental_model: None,
				text: "fact".into(),
				kind: Kind::World,
				learning: Learning::Fact,
				verification: Verification::Recorded,
				occurred: None,
				entities: vec![],
				evidence: vec![Evidence::JournalRange {
					run_id: runs[0],
					from_seq: 1,
					through_seq: 1,
				}],
				links: vec![],
			},
			learned_at: chrono::Utc::now(),
			updated_at: chrono::Utc::now(),
			deleted: false,
			stale: false,
		};
		let mut lease = super::super::access::Lease::within(&store, &TEST_FIXTURE)
			.await
			.unwrap();
		assert_eq!(
			get(&mut **lease.tx(), "acme").await.unwrap().0,
			SharingScope::Workspace
		);
		assert_eq!(
			set(&mut **lease.tx(), "acme", SharingScope::TenantAgent, 0)
				.await
				.unwrap(),
			1
		);
		assert!(agent_unit(&mut lease, &unit, "a", 16).await.unwrap());
		assert!(
			!agent_unit(&mut lease, &unit, "other-agent", 16)
				.await
				.unwrap()
		);
		unit.content.evidence.push(Evidence::JournalRange {
			run_id: runs[1],
			from_seq: 1,
			through_seq: 1,
		});
		assert!(!agent_unit(&mut lease, &unit, "a", 16).await.unwrap());
		lease.finish(Ok(())).await.unwrap();
	}
}
