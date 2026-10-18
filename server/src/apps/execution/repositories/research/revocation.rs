//! Evidence Revocation of all of one Run's Research evidence, and the audit
//! records it leaves (ADR 0046). A subject acts through its Tenant policy in
//! the authorization transaction; a Deployment Administrator needs none.
use super::{native, run_lock};
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchOriginal, ResearchPageImage, ResearchRevocation,
	ResearchSnapshot, ResearchSource, json_field, json_text, lock, revoke_invocations,
};
use crate::authorization::{access::NativeAccess, identity::Actor};
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
use chrono::SubsecRound;
use reinhardt::db::{backends::TransactionExecutor, orm::QueryRow, orm::execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, JoinType, LockBehavior, LockType, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder, TableRef,
};
use serde_json::json;
use uuid::Uuid;

/// The resource identifier of a Tenant's revocation records.
const RECORDS: &str = "revocations";

/// One Run's Context Journal, read through the revocation's own transaction.
/// That transaction already holds a connection and the Run's row, so a read
/// from the pool would need a second connection, which a small or busy pool
/// may never free while the revocation waits.
struct StoredJournal<'a> {
	tx: tokio::sync::Mutex<&'a mut dyn TransactionExecutor>,
	run: Uuid,
}

#[async_trait]
impl Journal for StoredJournal<'_> {
	async fn entries(&self, from: u64, through: u64) -> Result<Vec<HistoryEntry>> {
		let bound = |value: u64| {
			i64::try_from(value)
				.map_err(|_| Error::Invalid("context journal value is out of range".into()))
		};
		let (sql, values) = Query::select()
			.columns(["seq", "event"].map(Alias::new))
			.from(Alias::new(
				crate::apps::execution::repositories::context_journal::EVENTS,
			))
			.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
			.and_where(Expr::col("seq").gte(Expr::value(bound(from)?)))
			.and_where(Expr::col("seq").lte(Expr::value(bound(through)?)))
			.order_by(Alias::new("seq"), Order::Asc)
			.build(PostgresQueryBuilder);
		let rows = native(
			self.tx
				.lock()
				.await
				.fetch_all(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?;
		rows.into_iter()
			.map(|row| {
				let data = QueryRow::from_backend_row(row).data;
				let seq = data["seq"]
					.as_u64()
					.ok_or_else(|| Error::Invalid("context journal sequence is negative".into()))?;
				Ok(HistoryEntry {
					seq,
					event: serde_json::from_value(data["event"].clone())?,
				})
			})
			.collect()
	}
}

/// Apply a revoked Run's revocation to its stored projection, so every reader
/// of `runs.context`, local or remote, sees placeholders as soon as the
/// revocation commits. A Run whose worker holds its lease is scrubbed too, but
/// keeps its revision, which the worker's own keys derive from; the worker
/// applies the revocation before its next model request, and the retention
/// sweep catches a projection it saved meanwhile. A row a worker's step holds
/// locked is skipped rather than waited on, for the same reasons; the caller
/// retries it once its own transaction has committed.
/// `updated_at`, which dates the end of a terminal Run, is kept.
pub(crate) async fn scrub_projection(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<bool> {
	let (sql, values) = Query::select()
		.column(Alias::new("context"))
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.from(Alias::new("runs"))
		.lock(LockType::Update)
		.lock_behavior(LockBehavior::SkipLocked)
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
	let journal = StoredJournal {
		tx: tokio::sync::Mutex::new(&mut *tx),
		run,
	};
	revoke_projection(&mut context, &journal).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("context"), serde_json::to_value(&context)?)
		// A worker's live lease means its step saves next and moves the
		// revision itself.
		.value_expr(
			Alias::new("revision"),
			Expr::case()
				.when(
					Expr::col("lease_until")
						.is_null()
						.or(Expr::col("lease_until").lte(Expr::current_timestamp())),
					Expr::col("revision").add(1),
				)
				.else_result(Expr::col("revision")),
		)
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.build(PostgresQueryBuilder);
	native(
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from),
	)?;
	Ok(true)
}

/// Rounds of a scrub retried while a worker's step holds the Run's row, and the
/// pause between them. A Run still locked after them is left to the worker's
/// next step and to the next sweep.
const LOCKED_SCRUB_ROUNDS: u32 = 20;
const LOCKED_SCRUB_PAUSE: std::time::Duration = std::time::Duration::from_millis(50);

/// Scrub a revoked Run's projection in transactions of its own, retrying while
/// a worker's step holds its row: that step is about to commit a projection
/// read before the revocation, which the round after it scrubs. Returns
/// whether a round reached the row.
async fn scrub_retrying(store: &Store, run: Uuid) -> Result<bool> {
	for round in 0..LOCKED_SCRUB_ROUNDS {
		if round > 0 {
			tokio::time::sleep(LOCKED_SCRUB_PAUSE).await;
		}
		let mut tx = native(super::retention_transaction(store).await)?;
		let done = scrub_projection(tx.as_mut(), run).await?;
		native(tx.commit().await.map_err(crate::Error::from))?;
		if done {
			return Ok(true);
		}
	}
	Ok(false)
}

/// Revoked Runs examined per round trip of the sweep.
const SCRUB_BATCH: u64 = 64;

/// Scrub the stored projections of revoked Runs that still lack the
/// revocation, such as one a worker saved while it was being revoked. Every
/// such Run is examined in one pass, in pages ordered by id; one a worker's
/// step holds locked is retried at the end of the pass.
pub(crate) async fn scrub_revoked_projections(store: &Store) -> Result<u32> {
	let mut scrubbed = 0;
	let mut skipped = Vec::new();
	let mut after: Option<Uuid> = None;
	loop {
		let mut query = Query::select();
		query
			.column((Alias::new("r"), Alias::new("id")))
			.from_as(Alias::new("runs"), Alias::new("r"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("research_revocations"), Alias::new("v")),
				Expr::col((Alias::new("v"), Alias::new("run_id")))
					.equals((Alias::new("r"), Alias::new("id"))),
			)
			.and_where(
				Func::coalesce(vec![
					json_text(
						Expr::col((Alias::new("r"), Alias::new("context"))),
						"research_revoked",
					),
					Expr::value("false").into(),
				])
				.ne(Expr::value("true")),
			)
			.order_by((Alias::new("r"), Alias::new("id")), Order::Asc)
			.limit(SCRUB_BATCH);
		if let Some(after) = after {
			query.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).gt(Expr::value(after)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		let runs = native(
			async {
				let mut tx = super::retention_transaction(store).await?;
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
		let Some(last) = runs.last().copied() else {
			break;
		};
		after = Some(last);
		for run in &runs {
			let mut tx = native(super::retention_transaction(store).await)?;
			let done = scrub_projection(tx.as_mut(), *run).await?;
			native(tx.commit().await.map_err(crate::Error::from))?;
			if done {
				scrubbed += 1;
			} else {
				skipped.push(*run);
			}
		}
		if (runs.len() as u64) < SCRUB_BATCH {
			break;
		}
	}
	for run in skipped {
		scrubbed += u32::from(scrub_retrying(store, run).await?);
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
	Subject { access: Box<NativeAccess> },
	Administrator(Box<dyn TransactionExecutor>),
}

struct Scope<'a> {
	federation: &'a Federation,
	decider: Decider,
	tx: Transaction,
	/// The revoked Run whose projection a worker's step held locked, scrubbed
	/// again once the revocation commits.
	locked: Option<Uuid>,
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
					access: Box::new(NativeAccess::begin(&self.federation.store, identity).await?),
				},
			),
			Actor::DeploymentAdministrator => (
				Decider::DeploymentAdministrator {
					identity: self.administrator,
				},
				// A Deployment Administrator revokes any Tenant's Run, as its
				// other work without a Workspace does.
				Transaction::Administrator(native(
					async {
						self.federation
							.store
							.pool
							.begin(&self.actor.tenancy()?)
							.await?
							.into_executor()
					}
					.await,
				)?),
			),
		};
		Ok(Scope {
			federation: self.federation,
			decider,
			tx,
			locked: None,
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

	/// Remove the exact content from every Web disclosure approval of the Run,
	/// whatever its state, since the approval list shows it to the requester
	/// and approver; then close those still waiting for a decision, so a Run
	/// waiting on one continues, and without research, and those approved but
	/// not yet consumed, so no repeated call sends what they approved. A
	/// consumption racing this one updates the same row under its revision,
	/// so exactly one of the two takes effect.
	async fn close_disclosures(&mut self, run: Uuid) -> Result<u32> {
		let data = || Expr::col(Alias::new("data"));
		let of_run = || json_text(data(), "run_id").eq(Expr::value(run.to_string()));
		let (sql, values) = Query::update()
			.table(Alias::new("core_records"))
			// `data - 'content'`: the object without its content key.
			.value_expr(
				Alias::new("data"),
				data().sub(Expr::value("content").cast_as("text")),
			)
			.and_where(Expr::col("kind").eq(Expr::value(
				aidash_application::capabilities::approvals::DISCLOSURE,
			)))
			.and_where(json_field(data(), "content").is_not_null())
			.and_where(of_run())
			.build(PostgresQueryBuilder);
		native(
			self.executor()
				.execute(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?;
		let (sql, values) = Query::update()
			.table(Alias::new("core_records"))
			.value(Alias::new("state"), "closed")
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
			.and_where(Expr::col("kind").eq(Expr::value(
				aidash_application::capabilities::approvals::DISCLOSURE,
			)))
			.and_where(Expr::col("state").is_in(["pending", "approved"]))
			.and_where(of_run())
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
		let Transaction::Subject { access, .. } = &mut self.tx else {
			return Ok(());
		};
		// The subject must see the Run exactly as `GET /api/runs/{id}` decides,
		// in this transaction, so the dependency locks that decision takes
		// also cover the revocation; an invisible and a missing Run are the
		// same refusal.
		match crate::apps::identity::repositories::execution::details::visible_within(
			self.federation,
			access,
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
		// PostgreSQL keeps microseconds, so the first answer carries the time
		// a repeated revocation reads back.
		let at = revocation.revoked_at.trunc_subsecs(6);
		let tx = self.executor();
		// Research writes of the Run take the same lock, so an attempt in
		// flight records no evidence once this commits.
		native(lock(tx, &run_lock(run)).await)?;
		if let Some(first) = native(ResearchRevocation::find(tx, run).await)? {
			return Ok(first);
		}
		let (snapshots, snapshot_digests) = native(ResearchSnapshot::revoke(tx, run, at).await)?;
		native(ResearchOriginal::revoke(tx, run).await)?;
		native(ResearchPageImage::revoke(tx, run).await)?;
		let fragments = native(ResearchFragment::revoke(tx, run).await)?;
		let sources = native(ResearchSource::revoke(tx, run).await)?;
		let attempts = native(ResearchAttempt::revoke(tx, run).await)?;
		let invocations = native(revoke_invocations(tx, Some(run)).await)?;
		if !scrub_projection(self.executor(), run).await? {
			self.locked = Some(run);
		}
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
			revoked_at: at,
			..revocation.clone()
		};
		native(ResearchRevocation::record(self.executor(), &recorded).await)?;
		Ok(recorded)
	}
	async fn finish(
		self: Box<Self>,
		result: Result<EvidenceRevocation>,
	) -> Result<EvidenceRevocation> {
		let (store, locked) = (self.federation.store.clone(), self.locked);
		let recorded = self.complete(result).await?;
		// The revocation has committed; a failed retry leaves the projection
		// to the worker's next step and the sweep rather than failing it.
		if let Some(run) = locked
			&& let Err(error) = scrub_retrying(&store, run).await
		{
			tracing::warn!(run_id = %run, %error, "revoked projection was not scrubbed after commit");
		}
		Ok(recorded)
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
