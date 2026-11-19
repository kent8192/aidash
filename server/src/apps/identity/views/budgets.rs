//! Budgets (#1012, ADR 0143).
use super::super::{
	oidc::BrowserOrigin, serializers::budgets::BudgetUpdate,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use aidash_domain::budgets::Definition;
use reinhardt::{Depends, Path, Response, StatusCode, delete, get, http::ViewResult, post, put};
use uuid::Uuid;

#[get("/api/tenants/{tenant}/budgets", name = "budgets", auth = "protected")]
pub async fn list(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.budgets(actor, origin, tenant).await)
}

#[post(
	"/api/tenants/{tenant}/budgets",
	name = "budgets-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(definition): Json<Definition>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.create_budget(actor, origin, tenant, definition)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/budgets/{budget}",
	name = "budgets-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, budget)): Path<(String, Uuid)>,
	Json(update): Json<BudgetUpdate>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.update_budget(actor, origin, tenant, budget, update)
			.await,
	)
}

#[delete(
	"/api/tenants/{tenant}/budgets/{budget}",
	name = "budgets-delete",
	auth = "protected"
)]
pub async fn remove(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, budget)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::status(
		service
			.delete_budget(actor, origin, tenant, budget)
			.await
			.map(|()| StatusCode::NO_CONTENT),
	)
}
