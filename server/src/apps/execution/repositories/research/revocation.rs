//! Evidence Revocation of all of one Run's Research evidence, and the audit
//! records it leaves (ADR 0046). A subject acts through its Tenant policy in
//! the authorization transaction; a Deployment Administrator needs none.
use super::{native, run_lock};
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchRevocation, ResearchSnapshot, ResearchSource, lock,
	revoke_invocations,
};
use crate::authorization::{
	access::NativeAccess,
	identity::{Actor, SubjectIdentity},
};
use crate::federation::Federation;
use crate::store::Store;
use aidash_application::{
	Error, Result,
	ports::research::{RevocationRecordScope, RevocationRepository, RevocationScope},
	research::revocation::{Journal, revoke_projection},
};
use aidash_domain::{
	context::{Context, HistoryEntry},
	research::revocation::{EvidenceRevocation, Removed},
	retention::Decider,
};
use async_trait::async_trait;
use reinhardt::db::{backends::TransactionExecutor, orm::QueryRow, orm::execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	TableRef,
};
use serde_json::json;
use uuid::Uuid;

/// The resource identifier of a Tenant's revocation records.
const RECORDS: &str = "revocations";

/// One Run's Context Journal through the store.
struct StoredJournal<'a> {
	store: &'a Store,
	run: Uuid,
}

#[async_trait]
impl Journal for StoredJournal<'_> {
	async fn entries(&self, from: u64, through: u64) -> Result<Vec<HistoryEntry>> {
		native(self.store.context_journal(self.run, from, through).await)
	}
}

/// Apply a revoked Run's revocation to its stored projection, so every reader
/// of `runs.context`, local or remote, sees placeholders. A Run whose worker
/// holds its lease is left alone: the worker applies the revocation before its
/// next model request, and the retention sweep catches a projection it saved
/// meanwhile. `updated_at`, which dates the end of a terminal Run, is kept.
pub(crate) async fn scrub_projection(
	tx: &mut dyn TransactionExecutor,
	store: &Store,
	run: Uuid,
) -> Result<bool> {
	let (sql, values) = Query::select()
		.column(Alias::new("context"))
		.from(Alias::new("runs"))
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.and_where(Expr::cust(
			"(lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP)",
		))
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	let Some(row) = native(
		tx.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from),
	)?
	else {
		return Ok(false);
	};
	let mut context: Context =
		serde_json::from_value(QueryRow::from_backend_row(row).data["context"].clone())?;
	revoke_projection(&mut context, &StoredJournal { store, run }).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("context"), serde_json::to_value(&context)?)
		.value(Alias::new("revision"), Expr::cust("revision + 1"))
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.build(PostgresQueryBuilder);
	native(
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from),
	)?;
	Ok(true)
}

/// Scrub the stored projections of revoked Runs that still lack the
/// revocation, such as one a worker saved while it was being revoked.
pub(crate) async fn scrub_revoked_projections(store: &Store) -> Result<u32> {
	let (sql, values) = Query::select()
		.column((Alias::new("r"), Alias::new("id")))
		.from_as(Alias::new("runs"), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("research_revocations"), Alias::new("v")),
			Expr::col((Alias::new("v"), Alias::new("run_id")))
				.equals((Alias::new("r"), Alias::new("id"))),
		)
		.and_where(Expr::cust(
			"COALESCE(r.context->>'research_revoked', 'false') <> 'true'",
		))
		.and_where(Expr::cust(
			"(r.lease_until IS NULL OR r.lease_until <= CURRENT_TIMESTAMP)",
		))
		.limit(64)
		.build(PostgresQueryBuilder);
	let runs = native(
		async {
			let mut tx = store.database().begin().await?;
			let rows = tx.fetch_all(&sql, convert_values(values)).await?;
			tx.commit().await?;
			Ok(rows)
		}
		.await,
	)?
	.into_iter()
	.filter_map(|row| {
		QueryRow::from_backend_row(row).data["id"]
			.as_str()
			.and_then(|id| id.parse::<Uuid>().ok())
	})
	.collect::<Vec<_>>();
	let mut scrubbed = 0;
	for run in runs {
		let mut tx = native(store.database().begin().await.map_err(crate::Error::from))?;
		let done = scrub_projection(tx.as_mut(), store, run).await?;
		native(tx.commit().await.map_err(crate::Error::from))?;
		scrubbed += u32::from(done);
	}
	Ok(scrubbed)
}

pub(crate) struct Revocations<'a> {
	pub federation: &'a Federation,
	pub actor: &'a Actor,
	/// The dashboard identity of a Deployment Administrator acting in a
	/// browser; none for the deployment's API token.
	pub administrator: Option<Uuid>,
}

enum Transaction {
	Subject {
		access: NativeAccess,
		identity: SubjectIdentity,
	},
	Administrator(Box<dyn TransactionExecutor>),
}

struct Scope<'a> {
	federation: &'a Federation,
	decider: Decider,
	tx: Transaction,
}

impl Revocations<'_> {
	async fn open(&self) -> Result<Scope<'_>> {
		let (decider, tx) = match self.actor {
			Actor::Subject(identity) => (
				Decider::Subject {
					tenant: identity.tenant.clone(),
					subject: identity.subject.clone(),
				},
				Transaction::Subject {
					access: NativeAccess::begin(&self.federation.store, identity).await?,
					identity: identity.clone(),
				},
			),
			Actor::Operator => (
				Decider::DeploymentAdministrator {
					identity: self.administrator,
				},
				Transaction::Administrator(native(
					self.federation
						.store
						.database()
						.begin()
						.await
						.map_err(Into::into),
				)?),
			),
		};
		Ok(Scope {
			federation: self.federation,
			decider,
			tx,
		})
	}
}

impl Scope<'_> {
	fn executor(&mut self) -> &mut dyn TransactionExecutor {
		match &mut self.tx {
			Transaction::Subject { access, .. } => access.tenant_transaction().0,
			Transaction::Administrator(tx) => tx.as_mut(),
		}
	}

	async fn complete<T>(self, result: Result<T>) -> Result<T> {
		match self.tx {
			Transaction::Subject { access, .. } => access
				.finish(result.map_err(crate::Error::from))
				.await
				.map_err(Into::into),
			Transaction::Administrator(tx) => {
				native(
					async {
						match &result {
							Ok(_) => tx.commit().await?,
							Err(_) => tx.rollback().await?,
						}
						Ok(())
					}
					.await,
				)?;
				result
			}
		}
	}

	/// A subject must be permitted `action` on `resource`; a Deployment
	/// Administrator needs no Tenant policy.
	async fn permit(&mut self, kind: &str, id: &str, action: &str) -> Result<()> {
		let Transaction::Subject { access, .. } = &mut self.tx else {
			return Ok(());
		};
		let resource = access.resource(kind, id, json!({}));
		native(access.require(&resource, action).await)
	}

	/// Close the Run's Web disclosure approvals still waiting for a decision,
	/// so a Run waiting on one continues, and without research.
	async fn close_disclosures(&mut self, run: Uuid) -> Result<u32> {
		let (sql, values) = Query::update()
			.table(Alias::new("core_records"))
			.value(Alias::new("state"), "closed")
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.and_where(Expr::col("kind").eq(Expr::value(
				aidash_application::capabilities::approvals::DISCLOSURE,
			)))
			.and_where(Expr::col("state").eq(Expr::value("pending")))
			.and_where(Expr::cust_with_values(
				"(data->>'run_id') = ?",
				[run.to_string()],
			))
			.build(PostgresQueryBuilder);
		let closed = native(
			self.executor()
				.execute(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?
		.rows_affected;
		Ok(u32::try_from(closed).unwrap_or(u32::MAX))
	}
}

#[async_trait]
impl RevocationScope for Scope<'_> {
	fn decider(&self) -> Decider {
		self.decider.clone()
	}
	async fn run_exists(&mut self, run: Uuid) -> Result<bool> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		Ok(native(
			self.executor()
				.fetch_optional(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?
		.is_some())
	}
	async fn require(&mut self, run: Uuid, action: &str) -> Result<()> {
		let Transaction::Subject { identity, .. } = &self.tx else {
			return Ok(());
		};
		// The subject must see the Run exactly as `GET /api/runs/{id}` decides;
		// an invisible and a missing Run are the same refusal.
		match aidash_application::authorization::run_details::visible(
			&crate::bootstrap::run_details_repository(self.federation, identity),
			run,
		)
		.await
		{
			Ok(_) => {}
			Err(Error::NotFound(_) | Error::Forbidden) => return Err(Error::Forbidden),
			Err(error) => return Err(error),
		}
		self.permit("research", &run.to_string(), action).await
	}
	async fn tenant(&mut self, run: Uuid) -> Result<Option<String>> {
		let (sql, values) = Query::select()
			.column((Alias::new("w"), Alias::new("tenant")))
			.from_as(Alias::new("runs"), Alias::new("r"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("authorization_workspaces"), Alias::new("w")),
				Expr::col((Alias::new("w"), Alias::new("workspace_id")))
					.equals((Alias::new("r"), Alias::new("workspace_id"))),
			)
			.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		let row = native(
			self.executor()
				.fetch_optional(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?;
		Ok(row.and_then(|row| {
			QueryRow::from_backend_row(row).data["tenant"]
				.as_str()
				.map(str::to_owned)
		}))
	}
	async fn revoke(&mut self, revocation: &EvidenceRevocation) -> Result<EvidenceRevocation> {
		let run = revocation.run_id;
		let at = revocation.revoked_at;
		let tx = self.executor();
		// Research writes of the Run take the same lock, so an attempt in
		// flight records no evidence once this commits.
		native(lock(tx, &run_lock(run)).await)?;
		if let Some(first) = native(ResearchRevocation::find(tx, run).await)? {
			return Ok(first);
		}
		let (snapshots, snapshot_digests) = native(ResearchSnapshot::revoke(tx, run, at).await)?;
		let fragments = native(ResearchFragment::revoke(tx, run).await)?;
		let sources = native(ResearchSource::revoke(tx, run).await)?;
		let attempts = native(ResearchAttempt::revoke(tx, run).await)?;
		let invocations = native(revoke_invocations(tx, Some(run)).await)?;
		let store = self.federation.store.clone();
		scrub_projection(self.executor(), &store, run).await?;
		self.close_disclosures(run).await?;
		let recorded = EvidenceRevocation {
			removed: Removed {
				sources,
				snapshots,
				fragments,
				attempts,
				invocations,
				snapshot_digests,
			},
			..revocation.clone()
		};
		native(ResearchRevocation::record(self.executor(), &recorded).await)?;
		Ok(recorded)
	}
	async fn finish(
		self: Box<Self>,
		result: Result<EvidenceRevocation>,
	) -> Result<EvidenceRevocation> {
		self.complete(result).await
	}
}

#[async_trait]
impl RevocationRecordScope for Scope<'_> {
	fn decider(&self) -> Decider {
		self.decider.clone()
	}
	async fn require(&mut self, _: &str, action: &str) -> Result<()> {
		self.permit("research", RECORDS, action).await
	}
	async fn records(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<EvidenceRevocation>> {
		native(ResearchRevocation::list(self.executor(), tenant, offset, limit).await)
	}
	async fn finish(self: Box<Self>, committed: bool) -> Result<()> {
		let result = if committed {
			Ok(())
		} else {
			Err(Error::Conflict("revocation records were not read".into()))
		};
		match self.complete(result).await {
			Err(Error::Conflict(_)) if !committed => Ok(()),
			other => other,
		}
	}
}

#[async_trait]
impl RevocationRepository for Revocations<'_> {
	async fn begin(&self) -> Result<Box<dyn RevocationScope + '_>> {
		Ok(Box::new(self.open().await?))
	}
	async fn begin_records(&self) -> Result<Box<dyn RevocationRecordScope + '_>> {
		Ok(Box::new(self.open().await?))
	}
	fn notify(&self) {
		self.federation.notify.notify_waiters();
	}
}
