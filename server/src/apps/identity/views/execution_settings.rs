use super::super::{
	oidc::BrowserOrigin, serializers::execution_settings::TenantExecutionSettingsUpdate,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, get, http::ViewResult, put};

#[get(
	"/api/tenants/{tenant}/execution-settings",
	name = "tenant-execution-settings",
	auth = "protected"
)]
pub async fn get(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.execution_settings(actor, origin, tenant, None)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/execution-settings",
	name = "tenant-execution-settings-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(update): Json<TenantExecutionSettingsUpdate>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.execution_settings(actor, origin, tenant, Some(update))
			.await,
	)
}

#[get(
	"/api/tenants/{tenant}/goal-judge-model",
	name = "tenant-goal-judge-model",
	auth = "protected"
)]
pub async fn judge_get(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.goal_judge_model(actor, origin, tenant, None).await)
}

#[put(
	"/api/tenants/{tenant}/goal-judge-model",
	name = "tenant-goal-judge-model-update",
	auth = "protected"
)]
pub async fn judge_update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(update): Json<super::super::serializers::execution_settings::GoalJudgeModel>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.goal_judge_model(actor, origin, tenant, Some(update))
			.await,
	)
}
