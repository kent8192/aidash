//! Host Access grants held until an address signs in.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "identity",
	table_name = "bootstrap_deployment_administrator_reservations"
)]
#[derive(Serialize, Deserialize)]
pub struct BootstrapDeploymentAdministratorReservation {
	#[field(primary_key = true, field_type = "text")]
	pub email: String,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field]
	pub created_at: DateTime<Utc>,
}

use super::{
	AdministrationActor, AdministrationEvent, DashboardDeploymentAdministratorGrant,
	DashboardIdentity, DemoReservation, database_time,
};
use crate::{Result, database::native::Pool};
use aidash_application::ports::authorization::dashboard::AccountPolicy;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// Deployment Administrator is Deployment-wide, so only an identity whose
/// issuer no Tenant Binding restricts may hold it (ADR 0102 amendment). Without
/// a sign-in policy no issuer is known to be unrestricted, so none is.
fn may_administer(policy: Option<&AccountPolicy>, issuer: &str) -> bool {
	policy.is_some_and(|policy| policy.unrestricted(issuer))
}

impl BootstrapDeploymentAdministratorReservation {
	pub(crate) async fn grant_or_hold(
		pool: &Pool,
		policy: Option<&AccountPolicy>,
		email: &str,
		identity: Option<Uuid>,
	) -> Result<&'static str> {
		pool.atomic(
			&TenancyScope::Deployment(DeploymentScopeReason::Bootstrap),
			async |tx| {
				lock_email(tx, email).await?;
				let reservation = Self::objects()
					.filter(Self::field_email().eq(email.to_owned()))
					.select_for_update()
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?
					.pop();
				let outcome;
				if let Some(id) = identity {
					let account = DashboardIdentity::objects()
						.filter(DashboardIdentity::field_id().eq(id))
						.all_with_db(tx)
						.await?
						.pop()
						.ok_or(crate::Error::Unauthorized)?;
					if account.disabled_at.is_some() {
						return Err(crate::Error::Forbidden);
					}
					// The address belongs to an identity of a Tenant-bound
					// issuer: grant nothing, and hold nothing it could claim.
					if !may_administer(policy, &account.issuer) {
						return Ok("refused: the identity's issuer is bound to a Tenant");
					}
					let existing = DashboardDeploymentAdministratorGrant::objects()
						.filter(DashboardDeploymentAdministratorGrant::field_identity_id().eq(id))
						.all_with_db(tx)
						.await?
						.pop();
					let changed = existing.as_ref().is_none_or(|g| !g.enabled);
					outcome = if changed {
						"granted"
					} else {
						"already granted"
					};
					if changed {
						DashboardDeploymentAdministratorGrant::change_enabled_in(
							tx,
							id,
							true,
							existing.map_or(0, |g| g.revision),
						)
						.await?;
						AdministrationActor::HostAccess
							.record(
								tx,
								AdministrationEvent {
									action: "deployment_administrator.grant",
									identity: Some(id),
									registration: None,
									mapping: None,
									tenant: None,
									subject: None,
									invitation: None,
								},
							)
							.await?;
					}
					if reservation.is_some() {
						Self::objects()
							.filter(Self::field_email().eq(email.to_owned()))
							.delete_with_conn(tx)
							.await?;
					}
				} else if reservation.is_none() {
					outcome = "reserved";
					let row = Self::build()
						.email(email.to_owned())
						.issuer("host_access".to_owned())
						.created_at(database_time(tx).await?)
						.finish();
					Self::objects().create_with_conn(tx, &row).await?;
				} else {
					outcome = "already reserved";
				}
				Ok(outcome)
			},
		)
		.await
	}

	/// Grants the Deployment Administrator held for `email` to `identity`, the
	/// identity signing in with it as its verified address. An identity that
	/// may not hold the role leaves the reservation for one that may.
	pub async fn consume(
		pool: &Pool,
		policy: Option<&AccountPolicy>,
		email: &str,
		identity: Uuid,
	) -> Result<()> {
		pool.atomic(
			&TenancyScope::Deployment(DeploymentScopeReason::Bootstrap),
			async |tx| {
				lock_email(tx, email).await?;
				let row = Self::objects()
					.filter(Self::field_email().eq(email.to_owned()))
					.select_for_update()
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?
					.pop();
				if row.is_none() {
					return Ok(());
				}
				let eligible = DashboardIdentity::objects()
					.filter(DashboardIdentity::field_id().eq(identity))
					.all_with_db(tx)
					.await?
					.pop()
					.is_some_and(|account| {
						account.disabled_at.is_none() && may_administer(policy, &account.issuer)
					});
				// Both Host Access grants go to one identity. A sign-in spends
				// the address's Tenant reservations first, so once another
				// identity holds one, this reservation waits for that identity
				// rather than splitting the roles between two accounts.
				let split = DemoReservation::objects()
					.filter(DemoReservation::field_email().eq(email.to_owned()))
					.filter(DemoReservation::field_source().eq("host_access".to_owned()))
					.all_with_db(tx)
					.await?
					.iter()
					.any(|reservation| reservation.spent_by.is_some_and(|by| by != identity));
				if !eligible || split {
					return Ok(());
				}
				let existing = DashboardDeploymentAdministratorGrant::objects()
					.filter(DashboardDeploymentAdministratorGrant::field_identity_id().eq(identity))
					.all_with_db(tx)
					.await?
					.pop();
				if existing.as_ref().is_none_or(|g| !g.enabled) {
					DashboardDeploymentAdministratorGrant::change_enabled_in(
						tx,
						identity,
						true,
						existing.map_or(0, |g| g.revision),
					)
					.await?;
					AdministrationActor::HostAccess
						.record(
							tx,
							AdministrationEvent {
								action: "deployment_administrator.grant",
								identity: Some(identity),
								registration: None,
								mapping: None,
								tenant: None,
								subject: None,
								invitation: None,
							},
						)
						.await?;
				}
				Self::objects()
					.filter(Self::field_email().eq(email.to_owned()))
					.delete_with_conn(tx)
					.await?;
				Ok(())
			},
		)
		.await
	}
}

/// Serializes reservation creation and consumption even when the row is absent.
async fn lock_email(tx: &mut dyn TransactionExecutor, email: &str) -> Result<()> {
	let hash = SimpleExpr::FunctionCall(
		"hashtextextended".into_iden(),
		vec![
			Expr::value(email.to_owned()).into(),
			Expr::value(892_895_i64).into(),
		],
	);
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![hash],
		))
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
	Ok(())
}
