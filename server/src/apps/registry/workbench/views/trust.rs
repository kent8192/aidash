//! Native HTTP endpoints.
use crate::apps::registry::workbench::serializers::trust::PermissionInput;
use crate::apps::registry::workbench::serializers::trust::ReportQuery;
use crate::apps::registry::workbench::trust::TrustInspection;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

#[get(
	"/api/workbench/versions/{id}/{version}",
	name = "workbench-inspect-version",
	auth = "protected"
)]
pub async fn inspect(
	#[inject] service: Depends<TrustInspection>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.inspect(actor, (id, version)).await)
}

#[post(
	"/api/workbench/versions/{id}/{version}/permissions",
	name = "workbench-permission-context",
	auth = "protected"
)]
pub async fn permission_context(
	#[inject] service: Depends<TrustInspection>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<PermissionInput>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.permission_context(actor, (id, version), input)
			.await,
	)
}

#[get(
	"/api/workbench/versions/{id}/{version}/report",
	name = "workbench-export-report",
	auth = "protected"
)]
pub async fn report(
	#[inject] service: Depends<TrustInspection>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
	Query(query): Query<ReportQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.report(actor, (id, version), query).await)
}
