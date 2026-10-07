//! Tenant-owned Marketplace HTTP endpoints.
use super::super::serializers::{contracts::*, management::*};
use super::super::services::management::MarketplaceManagement;
use crate::authorization::identity::Actor;
use crate::dashboard_auth::BrowserOrigin;
use crate::http::json::Json;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Query, Response, get, post, put};

#[get(
	"/api/marketplace/packages",
	name = "marketplace-browse",
	auth = "protected"
)]
pub async fn browse(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Query(input): Query<Browse>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.browse(actor, input).await)
}

#[get(
	"/api/marketplace/packages/{key}",
	name = "marketplace-detail",
	auth = "protected"
)]
pub async fn detail(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path(key): Path<String>,
) -> ViewResult<Response> {
	crate::http::response(service.detail(actor, key).await)
}

#[post(
	"/api/marketplace/packages",
	name = "marketplace-publish-registered",
	auth = "protected"
)]
pub async fn publish(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Json(input): Json<Publish>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.publish(actor, input).await)
}

#[get(
	"/api/marketplace/sources",
	name = "marketplace-sources",
	auth = "protected"
)]
pub async fn sources(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Query(input): Query<SourceQuery>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.sources(actor, input).await)
}

#[post(
	"/api/marketplace/packages/{key}/install",
	name = "marketplace-install-scoped",
	auth = "protected"
)]
pub async fn install(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path(key): Path<String>,
	Json(input): Json<Install>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.install(actor, key, input).await)
}

#[put(
	"/api/marketplace/packages/{key}/audience",
	name = "marketplace-share",
	auth = "protected"
)]
pub async fn share(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path(key): Path<String>,
	Json(input): Json<AudienceInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.share(actor, key, input).await)
}

#[put(
	"/api/marketplace/packages/{key}/consents/{tenant}",
	name = "marketplace-consent",
	auth = "protected"
)]
pub async fn consent(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path((key, tenant)): Path<(String, String)>,
	Json(input): Json<AudienceInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.consent(actor, (key, tenant), input).await)
}

#[get(
	"/api/marketplace/packages/{key}/consents/{tenant}",
	name = "marketplace-read-consent",
	auth = "protected"
)]
pub async fn read_consent(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path((key, tenant)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::response(service.read_consent(actor, (key, tenant)).await)
}

#[get(
	"/api/marketplace/installations",
	name = "marketplace-installations",
	auth = "protected"
)]
pub async fn list_installations(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::response(service.list_installations(actor).await)
}

#[get(
	"/api/marketplace/installations/{id}",
	name = "marketplace-installation",
	auth = "protected"
)]
pub async fn installation(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path(id): Path<String>,
	Query(query): Query<RevisionQuery>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&query) {
		return Ok(error.http_response());
	}
	crate::http::response(service.installation(actor, id, query).await)
}

#[post(
	"/api/marketplace/installations/{id}",
	name = "marketplace-configure",
	auth = "protected"
)]
pub async fn configure(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Path(id): Path<String>,
	Json(input): Json<Configure>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.configure(actor, id, input).await)
}

#[get(
	"/api/marketplace/compatibility",
	name = "marketplace-compatibility",
	auth = "protected"
)]
pub async fn compatibility(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::json(service.compatibility(actor).await)
}

#[put(
	"/api/marketplace/compatibility",
	name = "marketplace-set-compatibility",
	auth = "protected"
)]
pub async fn set_compatibility(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Json(input): Json<CompatibilityInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(service.set_compatibility(actor, browser, input).await)
}

#[post(
	"/api/marketplace/installations/{id}/activation",
	name = "marketplace-activate",
	auth = "protected"
)]
pub async fn activate(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Path(id): Path<String>,
	Json(input): Json<Activate>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(service.activate(actor, browser, id, input).await)
}

#[post(
	"/api/marketplace/adoptions",
	name = "marketplace-adopt",
	auth = "protected"
)]
pub async fn adopt(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Json(input): Json<Adopt>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(service.adopt(actor, browser, input).await)
}

#[post(
	"/api/marketplace/approval-sets",
	name = "marketplace-approve-set",
	auth = "protected"
)]
pub async fn approve_set(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Json(input): Json<aidash_application::marketplace::operations::approval_set::ApprovalSet>,
) -> ViewResult<Response> {
	crate::http::json(service.approve_set(actor, browser, input).await)
}

#[post(
	"/api/marketplace/host-packages",
	name = "marketplace-host-packages",
	auth = "protected"
)]
pub async fn host_packages(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Json(input): Json<aidash_application::marketplace::operations::host_packages::HostPackages>,
) -> ViewResult<Response> {
	crate::http::json(service.host_packages(actor, browser, input).await)
}

#[get(
	"/api/marketplace/administration",
	name = "marketplace-administration",
	auth = "protected"
)]
pub async fn administration(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<BrowserOrigin>,
	Query(query): Query<AdministrationQuery>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&query) {
		return Ok(error.http_response());
	}
	crate::http::response(service.administration(actor, browser, query).await)
}

#[post(
	"/api/marketplace/publication-access",
	name = "marketplace-publication-access",
	auth = "protected"
)]
pub async fn publication_access(
	#[inject] service: Depends<MarketplaceManagement>,
	#[inject] actor: Actor,
	Json(input): Json<Publish>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service.publication_access(actor, input).await)
}
