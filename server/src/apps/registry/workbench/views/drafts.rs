//! Native HTTP endpoints.
use crate::apps::registry::workbench::Drafts;
use crate::apps::registry::workbench::serializers::contracts::DraftPage;
use crate::apps::registry::workbench::serializers::contracts::{
	AdoptInput, ArchiveInput, CreateDraft, RevisionInput, SaveDraft, ShareInput, TransferInput,
};
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post, put};
use uuid::Uuid;

#[post(
	"/api/workbench/drafts",
	name = "workbench-create-draft",
	auth = "protected"
)]
pub async fn create(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Json(input): Json<CreateDraft>,
) -> ViewResult<Response> {
	crate::http::json(service.create(actor, input).await)
}

#[get(
	"/api/workbench/drafts",
	name = "workbench-list-drafts",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Query(page): Query<DraftPage>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, page).await)
}

#[get(
	"/api/workbench/drafts/{id}",
	name = "workbench-get-draft",
	auth = "protected"
)]
pub async fn get(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.get(actor, id).await)
}

#[put(
	"/api/workbench/drafts/{id}",
	name = "workbench-save-draft",
	auth = "protected"
)]
pub async fn save(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<SaveDraft>,
) -> ViewResult<Response> {
	crate::http::json(service.save(actor, id, input).await)
}

#[post(
	"/api/workbench/drafts/{id}/duplicate",
	name = "workbench-duplicate-draft",
	auth = "protected"
)]
pub async fn duplicate(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> ViewResult<Response> {
	crate::http::json(service.duplicate(actor, id, input).await)
}

#[post(
	"/api/workbench/agents/{id}/{version}/adopt",
	name = "workbench-adopt-agent",
	auth = "protected"
)]
pub async fn adopt(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<AdoptInput>,
) -> ViewResult<Response> {
	crate::http::json(service.adopt(actor, (id, version), input).await)
}

#[post(
	"/api/workbench/drafts/{id}/shares",
	name = "workbench-share-draft",
	auth = "protected"
)]
pub async fn share(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ShareInput>,
) -> ViewResult<Response> {
	crate::http::json(service.share(actor, id, input).await)
}

#[get(
	"/api/workbench/drafts/{id}/shares",
	name = "workbench-draft-shares",
	auth = "protected"
)]
pub async fn shares(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.shares(actor, id).await)
}

#[post(
	"/api/workbench/drafts/{id}/transfer",
	name = "workbench-transfer-draft",
	auth = "protected"
)]
pub async fn transfer(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<TransferInput>,
) -> ViewResult<Response> {
	crate::http::json(service.transfer(actor, id, input).await)
}

#[post(
	"/api/workbench/drafts/{id}/archive",
	name = "workbench-archive-draft",
	auth = "protected"
)]
pub async fn archive(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ArchiveInput>,
) -> ViewResult<Response> {
	crate::http::json(service.archive(actor, id, input).await)
}

#[post(
	"/api/workbench/drafts/{id}/validate",
	name = "workbench-validate-draft",
	auth = "protected"
)]
pub async fn validate(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> ViewResult<Response> {
	crate::http::json(service.validate(actor, id, input).await)
}

#[get(
	"/api/workbench/drafts/{id}/versions",
	name = "workbench-draft-versions",
	auth = "protected"
)]
pub async fn versions(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.versions(actor, id).await)
}

#[post(
	"/api/workbench/drafts/{id}/register",
	name = "workbench-register-draft",
	auth = "protected"
)]
pub async fn register(
	#[inject] service: Depends<Drafts>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> ViewResult<Response> {
	crate::http::json(service.register(actor, id, input).await)
}
