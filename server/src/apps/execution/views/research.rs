//! Research evidence retention and revocation endpoints (ADR 0045, ADR 0046).
//! Subjects act through their Tenant policy; Deployment Administrators need none.
use crate::apps::execution::serializers::research::{HistoryPage, RetentionInput, RevocationInput};
use crate::apps::execution::services::management::HarnessManagement;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Query, Response, get, post, put};
use uuid::Uuid;

/// Revoke all of the Run's Research evidence, irreversibly. Needs a reason; a
/// subject needs `research.revoke` on the Run's `research` resource.
#[post(
	"/api/runs/{id}/research-revocation",
	name = "run-research-revocation",
	auth = "protected"
)]
pub async fn revoke(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	Path(id): Path<Uuid>,
	Json(input): Json<RevocationInput>,
) -> ViewResult<Response> {
	crate::http::json(service.research_revocation(actor, browser, id, input).await)
}

/// The Tenant's Evidence Revocation records, newest first.
#[get(
	"/api/tenants/{tenant}/research-revocations",
	name = "research-revocation-list",
	auth = "protected"
)]
pub async fn revocations(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<HistoryPage>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.research_revocations(actor, browser, tenant, page)
			.await,
	)
}

/// The Deployment's periods, the Tenant's own and those that apply.
#[get(
	"/api/tenants/{tenant}/retention",
	name = "tenant-retention",
	auth = "protected"
)]
pub async fn retention(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.retention(actor, browser, tenant).await)
}

/// Store or clear the Tenant's own periods, never longer than the Deployment's.
#[put(
	"/api/tenants/{tenant}/retention",
	name = "tenant-retention-update",
	auth = "protected"
)]
pub async fn retention_update(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<RetentionInput>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.retention_update(actor, browser, tenant, input)
			.await,
	)
}

/// The history of the Tenant's periods, newest first.
#[get(
	"/api/tenants/{tenant}/retention-history",
	name = "tenant-retention-history",
	auth = "protected"
)]
pub async fn retention_history(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<HistoryPage>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.retention_history(actor, browser, tenant, page)
			.await,
	)
}
