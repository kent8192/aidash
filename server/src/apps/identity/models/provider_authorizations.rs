//! Pending Provider Authorizations. Each row binds one OAuth `state` to the
//! browser that started it, its dashboard session, its Tenant and the
//! Provider Credential it will write. A callback consumes the row exactly once.
use super::dashboard_administration::database_time;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Hashes are hex SHA-256 digests; the plaintext `state` and browser binding
/// exist only in the provider redirect and the browser's cookie.
#[model(app_label = "identity", table_name = "provider_authorizations")]
#[derive(Serialize, Deserialize)]
pub struct ProviderAuthorization {
	#[field(primary_key = true, field_type = "text")]
	pub state_hash: String,
	#[field(field_type = "text")]
	pub browser_hash: String,
	#[field]
	pub session_id: Uuid,
	/// The dashboard context selector (`operator` or `mapping:<id>`) under
	/// which the callback re-derives and re-authorizes the actor.
	#[field(field_type = "text")]
	pub context: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field]
	pub provider_credential_id: Uuid,
	/// Present only for a reconnect, which rotates at this revision.
	#[field(null = true)]
	pub expected_revision: Option<i64>,
	#[field(field_type = "text")]
	pub pkce_verifier: String,
	#[field(field_type = "text")]
	pub callback_uri: String,
	#[field(field_type = "text")]
	pub return_to: String,
	#[field]
	pub expires_at: DateTime<Utc>,
}

const MAX_PENDING: i64 = 10_000;
const MAX_PENDING_PER_BROWSER: i64 = 8;

impl ProviderAuthorization {
	/// Stores this pending authorization. It supersedes any earlier pending
	/// authorization of the same browser, dashboard session and provider, so a
	/// declined consent, which arrives without `state`, always maps to exactly
	/// one row; the superseded flow's `state` is refused before any exchange.
	pub(crate) async fn reserve(&self, db: DatabaseConnection) -> Result<()> {
		db.atomic(async |tx| {
			let (sql, values) = Query::select()
				.expr(SimpleExpr::FunctionCall(
					Alias::new("pg_advisory_xact_lock").into_iden(),
					vec![Expr::value(71_003_258_i64).into()],
				))
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_expires_at().lte(now))
				.delete_with_conn(tx)
				.await?;
			Self::objects()
				.filter(Self::field_browser_hash().eq(self.browser_hash.clone()))
				.filter(Self::field_provider().eq(self.provider.clone()))
				.filter(Self::field_session_id().eq(self.session_id))
				.delete_with_conn(tx)
				.await?;
			let total = Self::objects().all().count_with_db(tx).await?;
			let pending = Self::objects()
				.filter(Self::field_browser_hash().eq(self.browser_hash.clone()))
				.count_with_db(tx)
				.await?;
			if total >= MAX_PENDING as usize || pending >= MAX_PENDING_PER_BROWSER as usize {
				return Err(Error::Conflict(
					"too many pending Provider Authorizations".into(),
				));
			}
			Self::objects().create_with_conn(tx, self).await?;
			Ok(())
		})
		.await
	}

	/// Deletes and returns the pending row for `state_hash`. Consumption commits
	/// even when the caller then rejects the row, so a `state` never works twice.
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

	/// Deletes and returns the one unexpired pending row of a browser,
	/// provider and dashboard session; [`Self::reserve`] keeps at most one.
	/// Only a declined consent uses it, because OpenRouter omits `state` from
	/// that redirect; the row never reaches an exchange.
	pub(crate) async fn consume_declined(
		db: DatabaseConnection,
		browser_hash: String,
		provider: &str,
		session_id: Uuid,
	) -> Result<Self> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			let mut rows = Self::objects()
				.filter(Self::field_browser_hash().eq(browser_hash))
				.filter(Self::field_provider().eq(provider.to_owned()))
				.filter(Self::field_session_id().eq(session_id))
				.filter(Self::field_expires_at().gt(now))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			// Never guess between flows: an ambiguous denial consumes nothing.
			let row = match (rows.pop(), rows.is_empty()) {
				(Some(row), true) => row,
				_ => return Err(Error::Unauthorized),
			};
			Self::objects()
				.filter(Self::field_state_hash().eq(row.state_hash.clone()))
				.delete_with_conn(tx)
				.await?;
			Ok(row)
		})
		.await
	}
}
