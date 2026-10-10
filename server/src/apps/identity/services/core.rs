use reinhardt::query::{Alias, Expr, LockType, Order, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
#[path = "access.rs"]
pub mod access;
#[path = "catalog.rs"]
pub mod catalog;
#[path = "execution.rs"]
pub mod execution;
#[path = "identity.rs"]
pub mod identity;
#[path = "interaction.rs"]
pub mod interaction;
#[path = "peer.rs"]
pub mod peer;
pub use super::policy;
#[path = "remote.rs"]
pub mod remote;
#[path = "resources.rs"]
pub mod resources;
#[path = "workspace.rs"]
pub mod workspace;

use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationDecision, AuthorizationRevision,
};
use crate::database::native::Pool;
use crate::{Error, Result};
use policy::{Decision, Evaluation, PolicyBundle, identifier};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use serde_json::Value;

#[derive(Clone)]
pub struct Authorization {
	pub pool: Pool,
}

impl Authorization {
	pub(crate) async fn evaluate_native(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		let mut scope = crate::apps::identity::repositories::NativePolicyScope(tx);
		Ok(
			aidash_application::authorization::Authorization::evaluate_in(
				&mut scope, tenant, input,
			)
			.await?,
		)
	}

	// Scope ORM reads and compound transactions to the application's native connection.
	fn native_connection(&self) -> Result<DatabaseConnectionLease> {
		Ok(DatabaseConnectionLease::register(self.pool.connection())?)
	}

	pub async fn replace(
		&self,
		tenant: &str,
		expected_revision: i64,
		bundle: PolicyBundle,
		actor: &str,
	) -> Result<Snapshot> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.replace(tenant, expected_revision, bundle, actor)
			.await?)
	}

	/// The protected tenant-creation route stages configured packages in the
	/// policy transaction. Updates and process restarts never enter this path.
	pub(crate) async fn replace_with_host_defaults(
		&self,
		store: &crate::store::Store,
		groups: &[String],
		tenant: &str,
		expected_revision: i64,
		bundle: PolicyBundle,
		actor: &str,
	) -> Result<(
		Snapshot,
		Option<aidash_application::marketplace::operations::host_packages::PendingHostPackages>,
	)> {
		aidash_application::authorization::validate_replacement(
			tenant,
			expected_revision,
			&bundle,
			actor,
		)?;
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let revision = AuthorizationBundle::replace(
			&mut tx,
			tenant,
			expected_revision,
			serde_json::to_value(&bundle)?,
			actor,
		)
		.await?;
		let pending = if expected_revision == 0 && !groups.is_empty() {
			let validation = crate::bootstrap::registry_validation_for(store);
			Some(
				aidash_application::marketplace::operations::host_packages::provision(
					&mut crate::bootstrap::marketplace_operator_scope(
						store,
						&mut tx,
						aidash_domain::identity::Principal::Operator,
					),
					&validation,
					&validation,
					&aidash_application::marketplace::operations::host_packages::HostPackages {
						tenant: tenant.into(),
						groups: groups.to_vec(),
						idempotency_key: uuid::Uuid::new_v4(),
					},
					&store.node_id,
				)
				.await?,
			)
		} else {
			None
		};
		tx.commit().await?;
		Ok((Snapshot { revision, bundle }, pending))
	}

	pub(crate) async fn load(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
	) -> Result<Snapshot> {
		Self::load_with_mode(tx, tenant, false).await
	}

	pub(crate) async fn load_with_mode(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		exclusive: bool,
	) -> Result<Snapshot> {
		identifier(tenant)?;
		let query = if exclusive {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder)
		} else {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder)
		};
		let row: Option<(i64, Value)> = crate::database::native::query_as(&query)
			.columns(&["revision", "document"])
			.bind(tenant)
			.fetch_optional(&mut **tx)
			.await?;
		let (revision, document) =
			row.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
		Ok(Snapshot {
			revision,
			bundle: serde_json::from_value(document)?,
		})
	}

	pub async fn snapshot(&self, tenant: &str) -> Result<Snapshot> {
		identifier(tenant)?;
		let lease = self.native_connection()?;
		lease
			.handle()
			.atomic(async |tx| AuthorizationBundle::lock_snapshot(tx, tenant, false).await)
			.await
	}

	pub async fn evaluate_in_transaction(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		let mut scope = crate::apps::identity::repositories::SqlPolicyScope(tx);
		Ok(
			aidash_application::authorization::Authorization::evaluate_in(
				&mut scope, tenant, input,
			)
			.await?,
		)
	}

	pub(crate) async fn record(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Self::record_many(tx, tenant, &[(input.clone(), decision.clone())]).await
	}

	/// Retain every decision and its allocation/commit order, while acquiring
	/// the transaction-wide audit controls only once for a compound check.
	pub(super) async fn record_many(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		records: &[(Evaluation, Decision)],
	) -> Result<()> {
		if records.is_empty() {
			return Ok(());
		}
		crate::transactions::authority::control(tx).await?;
		// Bound parameter count even for large replay/authorization operations.
		for records in records.chunks(100) {
			let columns = [
				"tenant",
				"revision",
				"subject",
				"action",
				"resource_kind",
				"resource_id",
				"decision",
			];
			let mut values = Query::select();
			for index in 0..records.len() {
				let mut row = Query::select();
				for (column, name) in columns.iter().enumerate() {
					let ty = match column {
						1 => "bigint",
						6 => "jsonb",
						_ => "text",
					};
					row.expr_as(
						Expr::cust(format!("${}::{ty}", index * 7 + column + 1)),
						Alias::new(*name),
					);
				}
				// This literal is an internal ordering index, not an SQLx parameter.
				row.expr_as(Expr::cust(index.to_string()), Alias::new("ordinal"));
				if index == 0 {
					values = row;
				} else {
					values.union_all(row);
				}
			}
			// The INSERT's source must read this materialized barrier before
			// defaults allocate any audit sequence. Retain the lock until commit,
			// but avoid a client round trip between acquiring it and inserting.
			crate::database::native::query(
				&Query::select()
					.expr(Expr::cust("pg_advisory_xact_lock(71003202)"))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?;
			let mut insert = Query::insert();
			insert
				.into_table(Alias::new("authorization_decisions"))
				.columns(columns.map(Alias::new))
				.from_subquery(
					Query::select()
						.columns(columns.map(|name| (Alias::new("decisions"), Alias::new(name))))
						.from_subquery(values, Alias::new("decisions"))
						.order_by((Alias::new("decisions"), Alias::new("ordinal")), Order::Asc)
						.to_owned(),
				);
			let sql = insert.to_string(PostgresQueryBuilder);
			let mut query = crate::database::native::query(&sql);
			for (input, decision) in records {
				query = query
					.bind(tenant)
					.bind(decision.revision)
					.bind(&input.subject)
					.bind(&input.action)
					.bind(&input.resource.kind)
					.bind(&input.resource.id)
					.bind(serde_json::to_value(decision)?);
			}
			query.execute(&mut **tx).await?;
		}
		Ok(())
	}

	pub async fn evaluate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.evaluate(tenant, input)
			.await?)
	}

	pub async fn simulate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.simulate(tenant, input)
			.await?)
	}

	pub async fn revisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		identifier(tenant)?;
		let lease = self.native_connection()?;
		AuthorizationRevision::page(&mut lease.handle(), tenant, after, limit).await
	}

	pub async fn decisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		identifier(tenant)?;
		let lease = self.native_connection()?;
		AuthorizationDecision::page(&mut lease.handle(), tenant, after, limit).await
	}
}

#[path = "oidc.rs"]
pub mod oidc;

pub use crate::apps::identity::serializers::contracts::Snapshot;
