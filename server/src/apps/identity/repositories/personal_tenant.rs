//! Atomic, idempotent Personal Tenant provisioning at sign-in. The caller has
//! checked the Tenancy Rules and that the identity has no Tenant Binding.
use super::tenants::{self, Created, Creator};
use crate::apps::identity::models::{DashboardIdentity, PersonalTenantLink, database_time};
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::tenancy::PERSONAL_LABEL;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{AtomicTransaction, Model, OrmExecutor, execution::convert_values},
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// Creates the identity's Personal Tenant unless it ever had one. The link is
/// written once at sign-in, so a disabled Mapping, a changed policy or a
/// deleted Personal Tenant is never repaired by signing in.
pub(crate) async fn provision(db: &Pool, identity: Uuid) -> Result<()> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::TenantCreation);
	db.atomic(&scope, async |tx| {
		lock(tx, identity).await?;
		if PersonalTenantLink::objects()
			.filter(PersonalTenantLink::field_identity_id().eq(identity))
			.exists_with_db(tx)
			.await?
		{
			return Ok(());
		}
		create(tx, identity, None).await.map(drop)
	})
	.await
}

/// Creates a new Personal Tenant for an identity that asks for one because its
/// Personal Tenant was deleted. The new one replaces the link and counts
/// toward no limit; the deleted one, if restored later, is no longer linked.
pub(crate) async fn recreate(
	db: &Pool,
	identity: Uuid,
	session: &crate::authorization::identity::HttpSession,
) -> Result<Created> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::TenantCreation);
	db.atomic(&scope, async |tx| {
		lock(tx, identity).await?;
		let link = PersonalTenantLink::objects()
			.filter(PersonalTenantLink::field_identity_id().eq(identity))
			.all_with_db(tx)
			.await?
			.pop();
		// The deletion row stays locked until the link moves, so a restore of
		// the old Tenant either committed before and is seen here, or waits.
		if let Some(link) = &link
			&& super::tenant_deletions::current_locked(tx, &link.tenant)
				.await?
				.is_none()
		{
			return Err(Error::Conflict("the Personal Tenant is not deleted".into()));
		}
		// The session at the write boundary: a logout committing meanwhile
		// waits, or is seen here.
		session.current(tx, true).await?;
		create(tx, identity, link).await
	})
	.await
}

/// Serializes one identity's Personal Tenant creations without taking an
/// identity row lock before policy authority. Approval and revocation retain
/// policy -> identity order.
async fn lock(tx: &mut AtomicTransaction, identity: Uuid) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(identity.to_string()).into(),
					Expr::value(71003248_i64).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
	Ok(())
}

/// Creates a Personal Tenant and links the identity to it, replacing `link`.
async fn create(
	tx: &mut AtomicTransaction,
	identity: Uuid,
	link: Option<PersonalTenantLink>,
) -> Result<Created> {
	if DashboardIdentity::find(tx, identity)
		.await?
		.disabled_at
		.is_some()
	{
		return Err(Error::Forbidden);
	}
	let created = tenants::create(tx, PERSONAL_LABEL, true, Creator::Identity(identity)).await?;
	let account = DashboardIdentity::objects()
		.filter(DashboardIdentity::field_id().eq(identity))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or(Error::Unauthorized)?;
	if account.disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	let now = database_time(tx).await?;
	match link {
		Some(mut link) => {
			link.tenant = created.name.clone();
			link.created_at = now;
			PersonalTenantLink::objects()
				.update_with_conn(tx, &link)
				.await?;
		}
		None => {
			let link = PersonalTenantLink::build()
				.identity_id(identity)
				.tenant(created.name.clone())
				.created_at(now)
				.finish();
			PersonalTenantLink::objects()
				.create_with_conn(tx, &link)
				.await?;
		}
	}
	Ok(created)
}

/// The Personal Tenant `identity` holds, if Aidash ever made one.
pub(crate) async fn of<E: OrmExecutor>(tx: &mut E, identity: Uuid) -> Result<Option<String>> {
	Ok(PersonalTenantLink::objects()
		.filter(PersonalTenantLink::field_identity_id().eq(identity))
		.all_with_db(tx)
		.await?
		.pop()
		.map(|link| link.tenant))
}
