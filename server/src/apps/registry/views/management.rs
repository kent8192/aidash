//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{
	authorization::identity::Actor,
	registry::{Entry, Package, Search},
};
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

use crate::apps::registry::serializers::management::InstallInput;
use crate::apps::registry::services::management::RegistryManagement;

#[get("/api/registry", name = "registry-list", auth = "protected")]
pub async fn registry_list(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	Query(search): Query<Search>,
) -> ViewResult<Response> {
	crate::http::response(service.registry_list(actor, search).await)
}

#[get(
	"/api/registry/{id}/{version}",
	name = "registry-get",
	auth = "protected"
)]
pub async fn registry_get(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::response(service.registry_get(actor, (id, version)).await)
}

#[post("/api/registry", name = "registry-create", auth = "protected")]
pub async fn registry_create(
	#[inject] service: Depends<RegistryManagement>,
	request: Request,
	Json(entry): Json<Entry>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.registry_create(request.headers.clone(), entry)
			.await,
	)
}

#[post("/api/skills/import", name = "skill-import", auth = "protected")]
pub async fn skill_import(
	#[inject] service: Depends<RegistryManagement>,
	Json(request): Json<crate::skill_import::ImportRequest>,
) -> ViewResult<Response> {
	crate::http::json(service.skill_import(request).await)
}

#[get("/api/marketplace", name = "marketplace", auth = "protected")]
pub async fn marketplace(
	#[inject] service: Depends<RegistryManagement>,
	Query(query): Query<Search>,
) -> ViewResult<Response> {
	crate::http::json(service.marketplace(query).await)
}

#[post("/api/marketplace", name = "package-publish", auth = "protected")]
pub async fn package_publish(
	#[inject] service: Depends<RegistryManagement>,
	Json(package): Json<Package>,
) -> ViewResult<Response> {
	crate::http::json(service.package_publish(package).await)
}

#[post(
	"/api/marketplace/{id}/{version}/install",
	name = "package-install",
	auth = "protected"
)]
pub async fn package_install(
	#[inject] service: Depends<RegistryManagement>,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<InstallInput>,
) -> ViewResult<Response> {
	crate::http::json(service.package_install((id, version), input).await)
}
