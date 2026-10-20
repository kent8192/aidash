//! Approval Mode and Approval Rule endpoints: a person's own, in one Tenant
//! (#411, ADR 0076). Every response describes one person's settings, so no
//! cache may keep it.
use super::super::{
	oidc::BrowserOrigin,
	serializers::approval_preferences::{PreferencesUpdate, RuleInput, ThreadModeUpdate},
	services::approval_preferences::ApprovalPreferences,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, StatusCode, delete, get, http::ViewResult, post, put};
use uuid::Uuid;

fn json<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|r| r.with_header("Cache-Control", "no-store"))
}

#[get(
	"/api/tenants/{tenant}/approval-preferences/mine",
	name = "approval-preferences",
	auth = "protected"
)]
pub async fn preferences(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	json(service.preferences(actor, origin, tenant).await)
}

#[put(
	"/api/tenants/{tenant}/approval-preferences/mine",
	name = "approval-preferences-update",
	auth = "protected"
)]
pub async fn update_preferences(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<PreferencesUpdate>,
) -> ViewResult<Response> {
	json(
		service
			.update_preferences(actor, origin, tenant, input)
			.await,
	)
}

#[get(
	"/api/tenants/{tenant}/approval-rules/mine",
	name = "approval-rules",
	auth = "protected"
)]
pub async fn rules(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	json(service.rules(actor, origin, tenant).await)
}

#[post(
	"/api/tenants/{tenant}/approval-rules/mine",
	name = "approval-rule-create",
	auth = "protected"
)]
pub async fn add_rule(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<RuleInput>,
) -> ViewResult<Response> {
	crate::http::json_status(
		service.add_rule(actor, origin, tenant, input).await,
		StatusCode::CREATED,
	)
	.map(|r| r.with_header("Cache-Control", "no-store"))
}

#[delete(
	"/api/tenants/{tenant}/approval-rules/mine/{id}",
	name = "approval-rule-delete",
	auth = "protected"
)]
pub async fn delete_rule(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::status(
		service
			.delete_rule(actor, origin, tenant, id)
			.await
			.map(|()| StatusCode::NO_CONTENT),
	)
}

#[get(
	"/api/tenants/{tenant}/subjects/{subject}/approval-settings",
	name = "approval-settings",
	auth = "protected"
)]
pub async fn settings_of(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, subject)): Path<(String, String)>,
) -> ViewResult<Response> {
	json(service.settings_of(actor, origin, tenant, subject).await)
}

#[get(
	"/api/workspaces/{workspace}/threads/{thread}/approval-mode",
	name = "thread-approval-mode",
	auth = "protected"
)]
pub async fn thread_mode(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	json(service.thread_mode(actor, origin, workspace, thread).await)
}

#[put(
	"/api/workspaces/{workspace}/threads/{thread}/approval-mode",
	name = "thread-approval-mode-update",
	auth = "protected"
)]
pub async fn set_thread_mode(
	#[inject] service: Depends<ApprovalPreferences>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
	Json(input): Json<ThreadModeUpdate>,
) -> ViewResult<Response> {
	json(
		service
			.set_thread_mode(actor, origin, workspace, thread, input)
			.await,
	)
}
