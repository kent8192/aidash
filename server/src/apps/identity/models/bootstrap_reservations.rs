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
	DashboardIdentity, database_time,
};
use crate::{Result, database::native::Pool};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

impl BootstrapDeploymentAdministratorReservation {
	pub(crate) async fn grant_or_hold(
		pool: &Pool,
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

	pub(crate) async fn consume(pool: &Pool, email: &str, identity: Uuid) -> Result<()> {
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
