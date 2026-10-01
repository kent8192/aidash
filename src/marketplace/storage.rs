use super::*;
use crate::{Error, Result, authorization::access::Access};
use sea_orm::sea_query::{Alias, Expr, OnConflict, Order, PostgresQueryBuilder, Query};
use serde::de::DeserializeOwned;
use sqlx::{Postgres, Transaction};

pub(super) async fn operator_begin(
	store: &crate::store::Store,
	browser: Option<&crate::dashboard_auth::BrowserOrigin>,
) -> Result<Transaction<'static, Postgres>> {
	let mut tx = store.pool.begin().await?;
	if let Some(browser) = browser {
		browser.require_operator(&mut tx, true).await?;
	}
	Ok(tx)
}

pub(super) async fn operator_commit(
	mut tx: Transaction<'static, Postgres>,
	browser: Option<&crate::dashboard_auth::BrowserOrigin>,
) -> Result<()> {
	if let Some(browser) = browser {
		browser.require_operator(&mut tx, false).await?;
	}
	tx.commit().await?;
	Ok(())
}

pub(super) fn key(value: &impl Serialize) -> String {
	crate::registry::digest(&serde_json::to_value(value).expect("serializable identity"))
		.trim_start_matches("sha256:")
		.to_string()
}
pub(super) async fn get<T: DeserializeOwned>(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	key: &str,
) -> Result<Option<T>> {
	let value: Option<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("document"))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("key")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(key)
	.fetch_optional(&mut **tx)
	.await?;
	value
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}
pub(super) async fn documents<T: DeserializeOwned>(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
) -> Result<Vec<T>> {
	let rows: Vec<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("document"))
			.from(Alias::new(table))
			.order_by(Alias::new("key"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **tx)
	.await?;
	rows.into_iter()
		.map(|v| serde_json::from_value(v).map_err(Into::into))
		.collect()
}
pub(super) async fn documents_page<T: DeserializeOwned>(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	after: &str,
	limit: u64,
) -> Result<Vec<(String, T)>> {
	let rows: Vec<(String, Value)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("key"), Alias::new("document")])
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("key")).gt(Expr::cust("$1")))
			.order_by(Alias::new("key"), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.bind(after)
	.fetch_all(&mut **tx)
	.await?;
	rows.into_iter()
		.map(|(key, value)| Ok((key, serde_json::from_value(value)?)))
		.collect()
}
pub(super) async fn put(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	key: &str,
	value: &impl Serialize,
) -> Result<()> {
	if table == "marketplace_installations" {
		let updated = sqlx::query(
			&Query::update()
				.table(Alias::new(table))
				.value(Alias::new("document"), Expr::cust("$2"))
				.and_where(Expr::cust("key=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(key)
		.bind(serde_json::to_value(value)?)
		.execute(&mut **tx)
		.await?;
		if updated.rows_affected() != 1 {
			return Err(Error::Forbidden);
		}
		return Ok(());
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new(table))
			.columns([Alias::new("key"), Alias::new("document")])
			.values_panic([Expr::cust("$1"), Expr::cust("$2")])
			.on_conflict(
				OnConflict::column(Alias::new("key"))
					.update_column(Alias::new("document"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(key)
	.bind(serde_json::to_value(value)?)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
/// One distribution lock orders audience/consent changes, absent-row creation,
/// publication and installation mappings. Credential/policy locks precede it;
/// catalog locks follow it. No network/provider work occurs inside this lease.
/// This deliberately favors a simple verifiable lock order over write throughput.
pub(crate) async fn lock(tx: &mut Transaction<'_, Postgres>, exclusive: bool) -> Result<()> {
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(if exclusive {
				"pg_advisory_xact_lock(74003201)"
			} else {
				"pg_advisory_xact_lock_shared(74003201)"
			}))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
pub(super) async fn writer(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(
				"set_config('aidash.marketplace_writer','1',true)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
pub(super) async fn gate(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
	let state: Compatibility = get(tx, "marketplace_gate", "v1")
		.await?
		.ok_or(Error::Forbidden)?;
	if !state.enabled || state.contract != 1 {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub(super) async fn begin(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	write: bool,
) -> Result<Access> {
	let mut access = Access::begin(store, identity).await?;
	access.marketplace_audit = Some(
		serde_json::json!({"request_id":Uuid::new_v4(),"tenant":identity.tenant,"actor":identity.subject,"credential_id":identity.credential_id,"policy_revision":access.snapshot.revision,"authority":{}}),
	);
	lock(&mut access.tx, write).await?;
	gate(&mut access.tx).await?;
	if write {
		writer(&mut access.tx).await?;
	}
	Ok(access)
}
#[derive(Serialize, Deserialize)]
struct Replay {
	fingerprint: String,
	result: Value,
}
pub(super) async fn replay(
	access: &mut Access,
	op: &str,
	id: Uuid,
	fingerprint: &str,
	node: &str,
) -> Result<Option<Value>> {
	let scope = key(&(
		access.identity.tenant.as_str(),
		access.identity.subject.as_str(),
		op,
		id,
	));
	let Some(saved): Option<Replay> = get(&mut access.tx, "marketplace_requests", &scope).await?
	else {
		return Ok(None);
	};
	if saved.fingerprint != fingerprint {
		// A changed target must not probe a result that has since been revoked.
		if op == "publish" {
			let version = super::distribution::load(
				access,
				saved.result["key"].as_str().ok_or(Error::Forbidden)?,
				"marketplace.read",
			)
			.await?;
			super::distribution::readable(access, &version, node).await?;
		} else {
			super::installations::view(
				access,
				saved.result["installation"]
					.as_str()
					.ok_or(Error::Forbidden)?,
				saved.result["revision"].as_i64(),
				node,
			)
			.await?;
		}
		return Err(Error::Conflict(
			"idempotency key has different input".into(),
		));
	}
	Ok(Some(saved.result))
}
pub(super) async fn remember(
	access: &mut Access,
	op: &str,
	id: Uuid,
	fingerprint: String,
	result: Value,
) -> Result<()> {
	let scope = key(&(
		access.identity.tenant.as_str(),
		access.identity.subject.as_str(),
		op,
		id,
	));
	put(
		&mut access.tx,
		"marketplace_requests",
		&scope,
		&Replay {
			fingerprint,
			result,
		},
	)
	.await
}
pub(super) fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}

/// Locks already protect revocation. Time-based expiry is checked again at the
/// actual handoff without reacquiring locks behind a waiting exclusive revoker.
pub(super) async fn credential_current(access: &mut Access) -> Result<()> {
	let valid: bool = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust(
				"expires_at>clock_timestamp() AND revoked_at IS NULL",
			))
			.from(Alias::new("authorization_credentials"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(access.identity.credential_id)
	.fetch_one(&mut **access.tx)
	.await?;
	if !valid {
		return Err(Error::Unauthorized);
	}
	access
		.identity
		.session_current(&mut access.tx, false)
		.await?;
	// Mapping/identity rows are already locked by Access. Only the clock can
	// advance the non-Google identity-status deadline while those locks are held.
	let current: Option<bool> = sqlx::query_scalar(&Query::select()
		.expr(Expr::cust("i.disabled_at IS NULL AND (i.issuer=$2 OR i.last_valid_at>clock_timestamp()-interval '15 minutes')"))
		.from_as(Alias::new("dashboard_mappings"), Alias::new("m"))
		.join_as(sea_orm::sea_query::JoinType::InnerJoin, Alias::new("dashboard_identities"), Alias::new("i"), Expr::cust("m.identity_id=i.id"))
		.and_where(Expr::cust("m.credential_id=$1")).to_string(PostgresQueryBuilder))
		.bind(access.identity.credential_id).bind(crate::config::GOOGLE_OIDC_ISSUER)
		.fetch_optional(&mut **access.tx).await?;
	if current == Some(false) {
		return Err(Error::IdentityStatusUnavailable);
	}
	Ok(())
}

/// Separate operator-only audit metadata; never policy input or a subject event.
pub(super) fn operation(access: &mut Access, name: &str, resource: Value, request: Option<Uuid>) {
	if let Some(audit) = &mut access.marketplace_audit
		&& audit.get("operation").is_none()
	{
		audit["operation"] = Value::String(name.into());
		audit["resource"] = resource;
		audit["idempotency_key"] = serde_json::json!(request);
	}
}
pub(super) fn authority(access: &mut Access, name: String, revision: i64) {
	if let Some(audit) = &mut access.marketplace_audit {
		audit["authority"][name] = serde_json::json!(revision);
	}
}
