//! Atomic, idempotent Personal Tenant provisioning at sign-in. The caller has
//! checked the Tenancy Rules and that the identity has no Tenant Binding.
use super::tenants::{self, Creator};
use crate::apps::identity::models::{DashboardIdentity, PersonalTenantLink, database_time};
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::tenancy::PERSONAL_LABEL;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, OrmExecutor, execution::convert_values},
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// Creates the identity's Personal Tenant unless it ever had one. The link is
/// written once, so a disabled Mapping, a changed policy or (later) a deleted
/// Personal Tenant is never repaired by signing in.
pub(crate) async fn provision(db: &Pool, node: &str, identity: Uuid) -> Result<()> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::TenantCreation);
	db.atomic(&scope, async |tx| {
		// Serialize first sign-ins without taking an identity row lock before
		// policy authority. Approval and revocation retain policy -> identity order.
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
		if PersonalTenantLink::objects()
			.filter(PersonalTenantLink::field_identity_id().eq(identity))
			.exists_with_db(tx)
			.await?
		{
			return Ok(());
		}
		if DashboardIdentity::find(tx, identity)
			.await?
			.disabled_at
			.is_some()
		{
			return Err(Error::Forbidden);
		}
		let created =
			tenants::create(tx, node, PERSONAL_LABEL, true, Creator::Identity(identity)).await?;
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
		let link = PersonalTenantLink::build()
			.identity_id(identity)
			.tenant(created.name)
			.created_at(database_time(tx).await?)
			.finish();
		PersonalTenantLink::objects()
			.create_with_conn(tx, &link)
			.await?;
		Ok(())
	})
	.await
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
