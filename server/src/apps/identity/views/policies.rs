//! HTTP endpoints backed by native dependency injection.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::policy::Evaluation;
use crate::apps::identity::serializers::policies::{
	AuthorizationPage, AuthorizationUpdate, CatalogApprovals, CatalogInput, CatalogQuery,
	CatalogReviewQuery, CredentialInput, CredentialPage,
};
use crate::apps::identity::services::policies::PolicyAdministration;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};
use uuid::Uuid;

#[get(
	"/api/authorization/{tenant}/catalog",
	name = "authorization-catalog",
	auth = "protected"
)]
pub async fn catalog(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: crate::authorization::identity::Actor,
	Path(tenant): Path<String>,
	Query(query): Query<CatalogQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.catalog(actor, tenant, query.node).await)
}

#[post(
	"/api/authorization/{tenant}/catalog",
	name = "authorization-set-catalog",
	auth = "protected"
)]
pub async fn set_catalog(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: crate::authorization::identity::Actor,
	Path(tenant): Path<String>,
	Json(input): Json<CatalogInput>,
) -> ViewResult<Response> {
	crate::http::json(service.set_catalog(actor, tenant, input).await)
}

#[post(
	"/api/authorization/{tenant}/catalog/approvals",
	name = "authorization-approve-catalog",
	auth = "protected"
)]
pub async fn approve_catalog(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: crate::authorization::identity::Actor,
	Path(tenant): Path<String>,
	Json(input): Json<CatalogApprovals>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.approve(actor, tenant, input.node, input.approvals)
			.await,
	)
}

#[get(
	"/api/authorization/{tenant}/catalog/review",
	name = "authorization-catalog-review",
	auth = "protected"
)]
pub async fn catalog_review(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: crate::authorization::identity::Actor,
	Path(tenant): Path<String>,
	Query(query): Query<CatalogReviewQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.review(actor, tenant, query).await)
}

#[post(
	"/api/authorization/{tenant}/catalog/review",
	name = "authorization-submit-catalog-review",
	auth = "protected"
)]
pub async fn submit_catalog_review(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: crate::authorization::identity::Actor,
	Path(tenant): Path<String>,
	Json(input): Json<aidash_application::authorization::catalog::review::ApprovalReviewSubmission>,
) -> ViewResult<Response> {
	crate::http::json(service.submit_review(actor, tenant, input).await)
}

#[post(
	"/api/authorization/{tenant}/credentials",
	name = "authorization-issue-credential",
	auth = "protected"
)]
pub async fn issue_credential(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<CredentialInput>,
) -> ViewResult<Response> {
	crate::http::json(service.issue_credential(actor, origin, tenant, input).await)
}

#[get(
	"/api/authorization/{tenant}/credentials",
	name = "authorization-credentials",
	auth = "protected"
)]
pub async fn credentials(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<CredentialPage>,
) -> ViewResult<Response> {
	crate::http::json(service.credentials(actor, origin, tenant, page).await)
}

#[post(
	"/api/authorization/{tenant}/credentials/{id}/revoke",
	name = "authorization-revoke-credential",
	auth = "protected"
)]
pub async fn revoke_credential(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	match service.revoke_credential(actor, origin, (tenant, id)).await {
		Ok((status, value)) => crate::http::json_status(Ok(value), status),
		Err(error) => Ok(error.http_response()),
	}
}

#[get(
	"/api/authorization/{tenant}",
	name = "authorization-snapshot",
	auth = "protected"
)]
pub async fn snapshot(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.snapshot(actor, origin, tenant).await)
}

#[post(
	"/api/authorization/{tenant}",
	name = "authorization-replace",
	auth = "protected"
)]
pub async fn replace(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<AuthorizationUpdate>,
) -> ViewResult<Response> {
	match service.replace(actor, origin, tenant, input).await {
		Ok((status, value)) => crate::http::json_status(Ok(value), status),
		Err(error) => Ok(error.http_response()),
	}
}

#[post(
	"/api/authorization/{tenant}/evaluate",
	name = "authorization-evaluate",
	auth = "protected"
)]
pub async fn evaluate(
	#[inject] service: Depends<PolicyAdministration>,
	Path(tenant): Path<String>,
	Json(input): Json<Evaluation>,
) -> ViewResult<Response> {
	crate::http::json(service.evaluate(tenant, input).await)
}

#[post(
	"/api/authorization/{tenant}/simulate",
	name = "authorization-simulate",
	auth = "protected"
)]
pub async fn simulate(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<Evaluation>,
) -> ViewResult<Response> {
	crate::http::json(service.simulate(actor, origin, tenant, input).await)
}

#[get(
	"/api/authorization/{tenant}/revisions",
	name = "authorization-revisions",
	auth = "protected"
)]
pub async fn revisions(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<AuthorizationPage>,
) -> ViewResult<Response> {
	crate::http::json(service.revisions(actor, origin, tenant, page).await)
}

#[get(
	"/api/authorization/{tenant}/decisions",
	name = "authorization-decisions",
	auth = "protected"
)]
pub async fn decisions(
	#[inject] service: Depends<PolicyAdministration>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<AuthorizationPage>,
) -> ViewResult<Response> {
	crate::http::json(service.decisions(actor, origin, tenant, page).await)
}

#[get(
	"/api/authorization/{tenant}/transaction-revocations",
	name = "!transaction_revocations",
	auth = "protected"
)]
pub async fn transaction_revocations(
	#[inject] service: Depends<PolicyAdministration>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.transaction_revocations(tenant).await)
}
