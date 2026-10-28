//! The audit record and replay receipt of a Deployment Administrator's Safety
//! Stop (ADR 0101). `run_id` has no foreign key: the record is audit history
//! and outlives the Run, its Workspace and the purge of its Tenant.
use crate::Result;
use aidash_application::safety_stop::{Receipt, Stored};
use aidash_domain::safety_stop::{Action, Actor, Command, Outcome, Record};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::model;
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "run_safety_stops")]
#[derive(Serialize, Deserialize)]
pub struct RunSafetyStop {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub action: String,
	#[field]
	pub expected_control_revision: i64,
	#[field(field_type = "text")]
	pub actor_kind: String,
	#[field(null = true)]
	pub identity_id: Option<Uuid>,
	#[field(field_type = "text")]
	pub reason: String,
	#[field]
	pub operation_id: Uuid,
	#[field(field_type = "text")]
	pub caller_key: String,
	#[field(field_type = "text")]
	pub fingerprint: String,
	#[field(field_type = "text")]
	pub outcome: String,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// Distinct from every other advisory lock namespace in the server.
const OPERATION_LOCK: i64 = 485_001;

impl RunSafetyStop {
	/// The earlier command of this caller with this operation ID, if any.
	///
	/// Takes a transaction-scoped lock on the operation key first. The Run lock
	/// serializes commands for one Run only; the same operation ID sent for two
	/// Runs of a Tenant would otherwise both miss here and the second insert
	/// would fail on the unique `(tenant, caller_key, operation_id)` index
	/// instead of answering the documented conflict.
	pub(crate) async fn receipt(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		caller_key: &str,
		operation: Uuid,
	) -> Result<Option<Stored>> {
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![
						Expr::value(format!("{tenant}\u{1f}{caller_key}\u{1f}{operation}")).into(),
						Expr::value(OPERATION_LOCK).into(),
					],
				)],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.filter(Self::field_caller_key().eq(caller_key.to_owned()))
			.filter(Self::field_operation_id().eq(operation))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| {
				Ok(Stored {
					fingerprint: row.fingerprint,
					receipt: Receipt {
						operation_id: row.operation_id,
						run_id: row.run_id,
						tenant: row.tenant,
						action: Action::try_from(row.action.as_str())?,
						outcome: Outcome::try_from(row.outcome.as_str())?,
					},
				})
			})
			.transpose()
	}

	/// Writes the audit entry and replay receipt of one command.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		actor: Actor,
		command: &Command,
		outcome: Outcome,
	) -> Result<()> {
		let record = Self::build()
			.id(Uuid::new_v4())
			.tenant(command.tenant.clone())
			.run_id(command.run_id)
			.action(command.action.as_str().to_owned())
			.expected_control_revision(command.expected_control_revision)
			.actor_kind(actor.kind().to_owned())
			.identity_id(actor.identity())
			.reason(command.reason.clone())
			.operation_id(command.operation_id)
			.caller_key(actor.caller_key())
			.fingerprint(command.fingerprint())
			.outcome(outcome.as_str().to_owned())
			.created_at(Utc::now())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// One Tenant's records, newest first.
	pub(crate) async fn list(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<Record>> {
		Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.order_by(&["-created_at", "id"])
			.offset(usize::try_from(offset).unwrap_or(usize::MAX))
			.limit(limit as usize)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|row| {
				Ok(Record {
					id: row.id,
					run_id: row.run_id,
					action: Action::try_from(row.action.as_str())?,
					actor_kind: row.actor_kind,
					identity_id: row.identity_id,
					reason: row.reason,
					operation_id: row.operation_id,
					outcome: Outcome::try_from(row.outcome.as_str())?,
					created_at: row.created_at,
				})
			})
			.collect()
	}
}
