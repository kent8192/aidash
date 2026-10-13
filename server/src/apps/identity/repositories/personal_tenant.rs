//! Atomic, idempotent private authority for direct Google sign-in.
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCredential, DashboardIdentity, DashboardMapping,
};
use crate::{Error, Result};
use aidash_domain::identity::personal_tenant::{OWNER, PersonalTenant};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{DatabaseConnection, Model, execution::convert_values},
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

fn require_google(identity: &DashboardIdentity) -> Result<()> {
	if !identity.direct_google() || identity.disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn provision(db: DatabaseConnection, identity: Uuid) -> Result<()> {
	let personal = PersonalTenant::for_identity(identity);
	db.atomic(async |tx| {
		// Serialize first sign-ins without taking an identity row lock before
		// policy authority. Approval and revocation retain policy -> identity order.
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![
						Expr::value(&personal.tenant).into(),
						Expr::value(71003248_i64).into(),
					],
				)],
			))
			.build(PostgresQueryBuilder);
		TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
		require_google(&DashboardIdentity::find(tx, identity).await?)?;
		let mappings = || {
			DashboardMapping::objects()
				.filter(DashboardMapping::field_identity_id().eq(identity))
				.filter(DashboardMapping::field_tenant().eq(personal.tenant.clone()))
				.filter(DashboardMapping::field_subject().eq(OWNER))
		};
		// Disabled mappings, withdrawn policies and expired/revoked credentials
		// are durable administrative decisions, never repaired by sign-in.
		if mappings().exists_with_db(tx).await? {
			return Ok(());
		}
		match AuthorizationBundle::lock_snapshot(tx, &personal.tenant, false).await {
			Ok(_) => {
				return Err(Error::Conflict(
					"personal tenant already exists without its owner mapping".into(),
				));
			}
			Err(Error::NotFound(_)) => {}
			Err(error) => return Err(error),
		}
		let bundle = personal.policy();
		bundle.validate()?;
		AuthorizationBundle::replace(
			tx,
			&personal.tenant,
			0,
			serde_json::to_value(bundle)?,
			"dashboard-personal-onboarding",
		)
		.await?;
		let account = DashboardIdentity::objects()
			.filter(DashboardIdentity::field_id().eq(identity))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or(Error::Unauthorized)?;
		require_google(&account)?;
		if mappings().exists_with_db(tx).await? {
			return Err(Error::Conflict("personal owner mapping changed".into()));
		}
		// This non-exported credential uses the existing dashboard marker so
		// credential listing and revocation continue to exclude browser secrets.
		let credential = AuthorizationCredential::issue(
			tx,
			&personal.tenant,
			OWNER,
			crate::apps::identity::services::oidc::digest(
				&crate::apps::identity::services::oidc::random_secret(),
			),
			3650 * 24 * 60 * 60,
			"dashboard-oidc",
		)
		.await?;
		let mapping = DashboardMapping::build()
			.id(Uuid::new_v4())
			.identity_id(identity)
			.tenant(personal.tenant.clone())
			.subject(OWNER.to_owned())
			.credential_id(credential.id)
			.enabled(true)
			.revision(1)
			.finish();
		DashboardMapping::objects()
			.create_with_conn(tx, &mapping)
			.await?;
		Ok(())
	})
	.await
}
