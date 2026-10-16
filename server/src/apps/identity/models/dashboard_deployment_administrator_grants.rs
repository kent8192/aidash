//! Persistent dashboard_deployment_administrator_grants records.

use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(
	app_label = "identity",
	table_name = "dashboard_deployment_administrator_grants"
)]
#[derive(Serialize, Deserialize)]
pub struct DashboardDeploymentAdministratorGrant {
	#[field(primary_key = true)]
	pub identity_id: uuid::Uuid,
	#[field(default = true)]
	pub enabled: bool,
	#[field(default = 1)]
	pub revision: i64,
}

impl DashboardDeploymentAdministratorGrant {
	pub(crate) async fn enabled_for_identity<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> crate::Result<bool> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_enabled().eq(true))
			.exists_with_db(db)
			.await?)
	}
}

impl DashboardDeploymentAdministratorGrant {}
