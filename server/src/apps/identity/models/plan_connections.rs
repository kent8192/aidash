//! Plan Connection metadata, pending Sign in with ChatGPT authorizations, Run
//! pins and this Deployment's host ID (#159, ADR 0007, ADR 0031). Plan Tokens
//! are never ORM fields; they stay in the Provider Credential Store.
use super::dashboard_administration::database_time;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, IntoIden, IntoValue, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One subject's Plan Connection in one Tenant, unique per provider.
#[model(app_label = "identity", table_name = "plan_connections")]
#[derive(Serialize, Deserialize)]
pub struct PlanConnection {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub provider: String,
	/// OpenAI's issued client ID; not secret, reused only for this connection.
	#[field(field_type = "text")]
	pub client_id: String,
	/// The verified ChatGPT account (`sub`) the grant was issued for.
	#[field(field_type = "text")]
	pub account: String,
	#[field(field_type = "text")]
	pub secret_resource: String,
	#[field(field_type = "text", null = true)]
	pub pinned_version: Option<String>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub access_expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub earliest_refresh_at: Option<DateTime<Utc>>,
	#[field]
	pub refresh_expires_at: DateTime<Utc>,
	#[field]
	pub connected_at: DateTime<Utc>,
	#[field(null = true)]
	pub refreshed_at: Option<DateTime<Utc>>,
	#[field]
	pub revision: i64,
}

/// A pending authorization. The plaintext `state` exists only in the
/// authorization URL and the callback URL; this row keeps its SHA-256.
#[model(app_label = "identity", table_name = "plan_authorizations")]
#[derive(Serialize, Deserialize)]
pub struct PlanAuthorization {
	#[field(primary_key = true, field_type = "text")]
	pub state_hash: String,
	/// The dashboard or desktop session that started it; only that session
	/// may complete it.
	#[field]
	pub session_id: Uuid,
	/// The dashboard context selector under which the actor is re-derived.
	#[field(field_type = "text")]
	pub context: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub provider: String,
	/// The connection's issued client on a reauthorization.
	#[field(field_type = "text", null = true)]
	pub saved_client: Option<String>,
	#[field(field_type = "text")]
	pub nonce: String,
	#[field(field_type = "text")]
	pub pkce_verifier: String,
	#[field(field_type = "text")]
	pub redirect_uri: String,
	#[field]
	pub expires_at: DateTime<Utc>,
}

/// Local-only admission evidence: the Plan Connection a Run's delegation-chain
/// root owned when the Run was admitted. Never serialized into Run or peer
/// messages, so Federation carries no Plan Connection data. Deleting the
/// connection deletes its pins, so the Run's next call fails.
#[model(app_label = "identity", table_name = "run_plan_connections")]
#[derive(Serialize, Deserialize)]
pub struct RunPlanConnection {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field]
	pub plan_connection_id: Uuid,
}

/// The Deployment's `ext_agent_host_id`: one random UUID, created once.
#[model(app_label = "identity", table_name = "plan_connection_hosts")]
#[derive(Serialize, Deserialize)]
pub struct PlanConnectionHost {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field]
	pub host_id: Uuid,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// Bounds pending rows so an authenticated caller cannot grow the table.
const MAX_PENDING: usize = 10_000;
/// Distinct from every other advisory lock namespace in the server.
const PENDING_LOCK: i64 = 159_002;
/// The only row of `plan_connection_hosts`.
const HOST_ROW: &str = "deployment";

impl PlanAuthorization {
	/// Stores this pending authorization. It supersedes any earlier pending
	/// authorization of the same session, Tenant, subject and provider, so
	/// only the latest attempt's callback URL completes.
	pub(crate) async fn reserve(&self, db: DatabaseConnection) -> Result<()> {
		db.atomic(async |tx| {
			let (sql, values) = Query::select()
				.expr(SimpleExpr::FunctionCall(
					"pg_advisory_xact_lock".into_iden(),
					vec![Expr::value(PENDING_LOCK).into()],
				))
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_expires_at().lte(now))
				.delete_with_conn(tx)
				.await?;
			Self::objects()
				.filter(Self::field_session_id().eq(self.session_id))
				.filter(Self::field_tenant().eq(self.tenant.clone()))
				.filter(Self::field_subject().eq(self.subject.clone()))
				.filter(Self::field_provider().eq(self.provider.clone()))
				.delete_with_conn(tx)
				.await?;
			if Self::objects().all().count_with_db(tx).await? >= MAX_PENDING {
				return Err(Error::Conflict(
					"too many pending Sign in with ChatGPT authorizations".into(),
				));
			}
			Self::objects().create_with_conn(tx, self).await?;
			Ok(())
		})
		.await
	}

	/// Deletes every pending authorization of the owner's provider, in any
	/// session: a disconnect ends them, so none of their callbacks can bring
	/// a connection back, a first-time attempt included.
	pub(crate) async fn cancel(
		db: DatabaseConnection,
		tenant: &str,
		subject: &str,
		provider: &str,
	) -> Result<()> {
		db.atomic(async |tx| {
			Self::objects()
				.filter(Self::field_tenant().eq(tenant.to_owned()))
				.filter(Self::field_subject().eq(subject.to_owned()))
				.filter(Self::field_provider().eq(provider.to_owned()))
				.delete_with_conn(tx)
				.await?;
			Ok(())
		})
		.await
	}

	/// Deletes and returns the pending row for `state_hash`. Consumption
	/// commits even when the caller then rejects the row, so a `state` never
	/// works twice.
	pub(crate) async fn consume(db: DatabaseConnection, state_hash: String) -> Result<Self> {
		db.atomic(async |tx| {
			let row = Self::objects()
				.filter(Self::field_state_hash().eq(state_hash))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Unauthorized)?;
			Self::objects()
				.filter(Self::field_state_hash().eq(row.state_hash.clone()))
				.delete_with_conn(tx)
				.await?;
			let now = database_time(tx).await?;
			Ok((row, now))
		})
		.await
		.and_then(|(row, now): (Self, DateTime<Utc>)| {
			if row.expires_at <= now {
				return Err(Error::Unauthorized);
			}
			Ok(row)
		})
	}
}

impl PlanConnectionHost {
	/// This Deployment's host ID, created on first use. Concurrent first uses
	/// agree on one row.
	pub(crate) async fn current(db: DatabaseConnection) -> Result<Uuid> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(["id", "host_id", "created_at"].map(Alias::new))
				.values_panic([
					IntoValue::into_value(HOST_ROW),
					IntoValue::into_value(Uuid::new_v4()),
					IntoValue::into_value(now),
				])
				.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			Self::objects()
				.filter(Self::field_id().eq(HOST_ROW.to_owned()))
				.all_with_db(tx)
				.await?
				.pop()
				.map(|row| row.host_id)
				.ok_or_else(|| Error::Invalid("Plan Connection host ID is unavailable".into()))
		})
		.await
	}
}
