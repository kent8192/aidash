//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{
	authorization::identity::Actor,
	registry::{Entry, Search},
};
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

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

#[get(
	"/api/registry/versions",
	name = "registry-versions",
	auth = "protected"
)]
pub async fn registry_versions(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	Query(page): Query<crate::apps::registry::serializers::contracts::VersionPage>,
) -> ViewResult<Response> {
	crate::http::json(service.registry_versions(actor, page).await)
}

#[post("/api/registry", name = "registry-create", auth = "protected")]
pub async fn registry_create(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	request: Request,
	Json(entry): Json<Entry>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.registry_create(actor, request.headers.clone(), entry)
			.await,
	)
}

#[post(
	"/api/registry/mcp-tool-hint",
	name = "registry-mcp-tool-hint",
	auth = "protected"
)]
pub async fn mcp_tool_hint(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	Json(request): Json<crate::apps::registry::serializers::contracts::McpToolHintRequest>,
) -> ViewResult<Response> {
	crate::http::json(service.mcp_tool_hint(actor, request).await)
}

#[post("/api/skills/import", name = "skill-import", auth = "protected")]
pub async fn skill_import(
	#[inject] service: Depends<RegistryManagement>,
	#[inject] actor: Actor,
	Json(request): Json<crate::skill_import::ImportRequest>,
) -> ViewResult<Response> {
	crate::http::json(service.skill_import(actor, request).await)
}
