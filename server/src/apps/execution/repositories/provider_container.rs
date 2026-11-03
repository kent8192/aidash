//! One durable OpenRouter container identity per Run; there is no delete API.
use super::super::models::{
	Run,
	states::{RunControl, RunPhase},
};
use crate::{Error, Result, store::Store};
use aidash_application::tenancy::TenancyScope;
use reinhardt::{
	core::exception::Error as FrameworkError,
	db::orm::{Json, Model},
};
use serde_json::{Value, json};
use uuid::Uuid;

impl Store {
	pub(crate) async fn provider_container(
		&self,
		tenant: &str,
		run: Uuid,
		network_policy: &Value,
	) -> Result<String> {
		aidash_domain::tool::provider_tools::validate_network_policy(network_policy)?;
		self.control_pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				let row = Run::objects()
					.filter(Run::field_tenant().eq(tenant))
					.filter(Run::field_id().eq(run))
					.select_for_update()
					.all_with_executor(tx)
					.await
					.map_err(FrameworkError::from)?
					.pop()
					.ok_or_else(|| Error::NotFound("Run".into()))?;
				if row.control != RunControl::Active
					|| matches!(
						row.phase,
						RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled
					) {
					return Err(Error::Invalid("PROVIDER_CONTAINER_RUN_ENDED".into()));
				}
				if let Some(pin) = row.provider_container {
					if pin.0["network_policy"] != *network_policy {
						return Err(Error::Invalid("PROVIDER_CONTAINER_NETWORK_CHANGED".into()));
					}
					return pin.0["id"]
						.as_str()
						.filter(|id| aidash_domain::tool::provider_tools::valid_container_id(id))
						.map(str::to_owned)
						.ok_or_else(|| Error::Invalid("invalid stored provider container".into()));
				}
				let id = format!("aidash_{}", run.simple());
				let pin = Some(Json(json!({"id":id,"network_policy":network_policy})));
				// Update only the pin, preserving the worker's revision/lease and concurrent inputs.
				Run::objects()
					.filter(Run::field_tenant().eq(tenant))
					.filter(Run::field_id().eq(run))
					.update_fields_with_conn(tx, [(Run::field_provider_container(), pin)])
					.await?;
				Ok(id)
			})
			.await
	}
}
