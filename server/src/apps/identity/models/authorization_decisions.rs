//! Persistent authorization_decisions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_decisions")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationDecision {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub action: String,
	#[field(field_type = "text")]
	pub resource_kind: String,
	#[field(field_type = "text")]
	pub resource_id: String,
	#[field]
	pub decision: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// The Run this check was made for, if any.
	#[field]
	pub run_id: Option<Uuid>,
	/// The Remote Grant this check was made for, if any.
	#[field]
	pub remote_grant_id: Option<Uuid>,
	/// Read Coalescing: a repeat of this allow adds no row (ADR 0116).
	#[field(default = false)]
	pub coalesced: bool,
}

/// Who a check was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DecisionOwner {
	Run(Uuid),
	RemoteGrant(Uuid),
}

impl DecisionOwner {
	fn run(owner: Option<Self>) -> Option<Uuid> {
		match owner {
			Some(Self::Run(id)) => Some(id),
			_ => None,
		}
	}
	fn remote_grant(owner: Option<Self>) -> Option<Uuid> {
		match owner {
			Some(Self::RemoteGrant(id)) => Some(id),
			_ => None,
		}
	}
}

/// The part of a recorded row that identifies a coalesced decision.
#[derive(Deserialize)]
struct RecordedDecision {
	run_id: Option<Uuid>,
	remote_grant_id: Option<Uuid>,
	subject: String,
	action: String,
	resource_kind: String,
	resource_id: String,
	revision: i64,
}

/// One Authorization Decision on its way to the table.
#[derive(Clone)]
pub(crate) struct DecisionRow {
	pub(crate) input: Evaluation,
	pub(crate) decision: Decision,
	pub(crate) owner: Option<DecisionOwner>,
	pub(crate) coalesced: bool,
}

impl DecisionRow {
	/// A decision that names no Run and is written every time it is made.
	pub(crate) fn direct(input: Evaluation, decision: Decision) -> Self {
		Self {
			input,
			decision,
			owner: None,
			coalesced: false,
		}
	}
	/// What identifies a coalesced row. Every coalesced row is an allow.
	fn key(&self) -> (Option<DecisionOwner>, &str, &str, &str, &str, i64) {
		(
			self.owner,
			&self.input.subject,
			&self.input.action,
			&self.input.resource.kind,
			&self.input.resource.id,
			self.decision.revision,
		)
	}
}

use crate::Result;
use crate::apps::identity::services::policy::{Decision, Evaluation};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::QueryRow;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoIden, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

impl AuthorizationDecision {
	pub(crate) async fn append(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Self::append_many(tx, tenant, &[(input.clone(), decision.clone())]).await
	}

	/// Keep every decision in evaluation order without a round trip per check.
	pub(crate) async fn append_many(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		records: &[(Evaluation, Decision)],
	) -> Result<()> {
		let rows: Vec<DecisionRow> = records
			.iter()
			.map(|(input, decision)| DecisionRow::direct(input.clone(), decision.clone()))
			.collect();
		Self::append_rows(tx, tenant, &rows).await
	}

	/// Drop the coalesced rows already recorded. This only reads, so a repeated
	/// check that finds nothing new needs no write, no second connection and no
	/// audit lock (ADR 0116).
	pub(crate) async fn unrecorded(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		rows: Vec<DecisionRow>,
	) -> Result<Vec<DecisionRow>> {
		let coalesced = || rows.iter().filter(|row| row.coalesced);
		let revisions: BTreeSet<i64> = coalesced().map(|row| row.decision.revision).collect();
		if revisions.is_empty() {
			return Ok(rows);
		}
		let runs: BTreeSet<Uuid> = coalesced()
			.filter_map(|row| DecisionOwner::run(row.owner))
			.collect();
		let grants: BTreeSet<Uuid> = coalesced()
			.filter_map(|row| DecisionOwner::remote_grant(row.owner))
			.collect();
		let mut owners = Condition::any();
		if !runs.is_empty() {
			owners = owners.add(Expr::col("run_id").is_in(runs.into_iter().map(Expr::value)));
		}
		if !grants.is_empty() {
			owners =
				owners.add(Expr::col("remote_grant_id").is_in(grants.into_iter().map(Expr::value)));
		}
		let (sql, values) = Query::select()
			.columns(
				[
					"run_id",
					"remote_grant_id",
					"subject",
					"action",
					"resource_kind",
					"resource_id",
					"revision",
				]
				.map(Alias::new),
			)
			.from(Alias::new("authorization_decisions"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("coalesced").eq(true))
			.and_where(Expr::col("revision").is_in(revisions.into_iter().map(Expr::value)))
			.cond_where(owners)
			.build(PostgresQueryBuilder);
		let mut recorded = BTreeSet::new();
		for row in tx.fetch_all(&sql, convert_values(values)).await? {
			let row: RecordedDecision =
				serde_json::from_value(QueryRow::from_backend_row(row).data)?;
			let owner = row
				.run_id
				.map(DecisionOwner::Run)
				.or(row.remote_grant_id.map(DecisionOwner::RemoteGrant));
			recorded.insert((
				owner,
				row.subject,
				row.action,
				row.resource_kind,
				row.resource_id,
				row.revision,
			));
		}
		Ok(rows
			.into_iter()
			.filter(|row| {
				if !row.coalesced {
					return true;
				}
				let (owner, subject, action, kind, id, revision) = row.key();
				!recorded.contains(&(
					owner,
					subject.to_owned(),
					action.to_owned(),
					kind.to_owned(),
					id.to_owned(),
					revision,
				))
			})
			.collect())
	}

	/// Write what is new among `rows`, reading first in the same transaction.
	pub(crate) async fn append_unrecorded(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		rows: Vec<DecisionRow>,
	) -> Result<()> {
		let rows = Self::unrecorded(tx, tenant, rows).await?;
		Self::append_rows(tx, tenant, &rows).await
	}

	pub(crate) async fn append_rows(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		rows: &[DecisionRow],
	) -> Result<()> {
		if rows.is_empty() {
			return Ok(());
		}
		super::authority::authority_control(tx).await?;
		// Acquire before any INSERT can allocate a sequence, and retain through
		// commit so a cursor never skips a concurrently admitted transaction. The
		// only cursor reader filters by Tenant, so Tenants need not wait for each other.
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![
					Expr::value(71003202_i32).cast_as("int4"),
					SimpleExpr::FunctionCall(
						"hashtext".into_iden(),
						vec![Expr::value(tenant).cast_as("text")],
					),
				],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let columns = [
			"tenant",
			"revision",
			"subject",
			"action",
			"resource_kind",
			"resource_id",
			"decision",
			"run_id",
			"remote_grant_id",
			"coalesced",
		];
		// Bound statement size and parameter count for large disclosure responses.
		// A full dashboard window normally fits in one batch, including its audit.
		for rows in rows.chunks(256) {
			let mut selected = Query::select();
			for (index, row) in rows.iter().enumerate() {
				let mut one = Query::select();
				// cast_as quotes type names; bigint's catalog name is int8.
				for (name, value) in [
					("tenant", Expr::value(tenant).cast_as("text")),
					(
						"revision",
						Expr::value(row.decision.revision).cast_as("int8"),
					),
					("subject", Expr::value(&row.input.subject).cast_as("text")),
					("action", Expr::value(&row.input.action).cast_as("text")),
					(
						"resource_kind",
						Expr::value(&row.input.resource.kind).cast_as("text"),
					),
					(
						"resource_id",
						Expr::value(&row.input.resource.id).cast_as("text"),
					),
					(
						"decision",
						Expr::value(serde_json::to_value(&row.decision)?).cast_as("jsonb"),
					),
					(
						"run_id",
						Expr::value(DecisionOwner::run(row.owner)).cast_as("uuid"),
					),
					(
						"remote_grant_id",
						Expr::value(DecisionOwner::remote_grant(row.owner)).cast_as("uuid"),
					),
					("coalesced", Expr::value(row.coalesced).cast_as("bool")),
				] {
					one.expr_as(value, Alias::new(name));
				}
				one.expr_as(
					Expr::value(i64::try_from(index).expect("bounded audit batch")).cast_as("int8"),
					Alias::new("ordinal"),
				);
				if index == 0 {
					selected = one;
				} else {
					selected.union_all(one);
				}
			}
			// Two checks of one Run may both find a row missing; one of them writes it.
			let (sql, values) = Query::insert()
				.into_table(Alias::new("authorization_decisions"))
				.columns(columns.map(Alias::new))
				.from_subquery(
					Query::select()
						.columns(columns.map(|name| (Alias::new("decisions"), Alias::new(name))))
						.from_subquery(selected, Alias::new("decisions"))
						.order_by((Alias::new("decisions"), Alias::new("ordinal")), Order::Asc)
						.to_owned(),
				)
				.on_conflict(OnConflict::new().do_nothing())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		Ok(())
	}
	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		after: i64,
		limit: i64,
	) -> Result<Vec<Value>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_sequence().gt(after))
			.order_by(&["sequence"])
			.limit(limit.clamp(1, 200) as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| {
				json!({
					"sequence": row.sequence, "tenant": row.tenant, "revision": row.revision,
					"subject": row.subject, "action": row.action, "resource_kind": row.resource_kind,
					"resource_id": row.resource_id, "decision": row.decision.0, "created_at": row.created_at,
					"run": row.run_id, "remote_grant": row.remote_grant_id,
				})
			})
			.collect())
	}
}
