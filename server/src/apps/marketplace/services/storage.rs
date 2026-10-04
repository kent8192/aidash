use super::*;
use crate::{Error, Result, authorization::access::Access};
use reinhardt::query::{Alias, Expr, JoinType, PostgresQueryBuilder, Query, TableRef};

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
pub(crate) async fn writer(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
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
pub(crate) async fn gate(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
	let state: Compatibility = get(tx, "marketplace_gate", "v1")
		.await?
		.ok_or(Error::Forbidden)?;
	if !aidash_domain::marketplace::installations::compatible(&state) {
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

/// Locks already protect revocation. Time-based expiry is checked again at the
/// actual handoff without reacquiring locks behind a waiting exclusive revoker.
pub(super) async fn credential_current(access: &mut Access) -> Result<()> {
	let valid: bool = {
		let query_bind_1 = access.identity.credential_id;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust(
					"expires_at>clock_timestamp() AND revoked_at IS NULL",
				))
				.from(Alias::new("authorization_credentials"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **access.tx)
		.await?
	};
	if !valid {
		return Err(Error::Unauthorized);
	}
	access
		.identity
		.session_current(&mut access.tx, false)
		.await?;
	// Mapping/identity rows are already locked by Access. Only the clock can
	// advance the non-Google identity-status deadline while those locks are held.
	let current: Option<bool> = {
		let query_bind_1 = access.identity.credential_id;
		let query_bind_2 = crate::config::GOOGLE_OIDC_ISSUER;
		sqlx::query_scalar(&Query::select()
		.expr(SimpleExpr::CustomWithExpr("(i.disabled_at IS NULL AND (i.issuer=? OR i.last_valid_at>clock_timestamp()-interval '15 minutes'))".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()]))
		.from_as(Alias::new("dashboard_mappings"), Alias::new("m"))
		.join(JoinType::InnerJoin, TableRef::table_alias(Alias::new("dashboard_identities"), Alias::new("i")), Expr::cust("m.identity_id=i.id"))
		.and_where(SimpleExpr::CustomWithExpr("(m.credential_id=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(PostgresQueryBuilder))
		.fetch_optional(&mut **access.tx).await?
	};
	if current == Some(false) {
		return Err(Error::IdentityStatusUnavailable);
	}
	Ok(())
}

/// Separate operator-only audit metadata; never policy input or a subject event.
pub(crate) fn operation(access: &mut Access, name: &str, resource: Value, request: Option<Uuid>) {
	if let Some(audit) = &mut access.marketplace_audit
		&& audit.get("operation").is_none()
	{
		audit["operation"] = Value::String(name.into());
		audit["resource"] = resource;
		audit["idempotency_key"] = serde_json::json!(request);
	}
}
pub(crate) fn authority(access: &mut Access, name: String, revision: i64) {
	if let Some(audit) = &mut access.marketplace_audit {
		audit["authority"][name] = serde_json::json!(revision);
	}
}

use reinhardt::query::QueryStatementBuilder as _;

pub(super) use crate::apps::marketplace::repositories::storage::get;

use reinhardt::query::SimpleExpr;
