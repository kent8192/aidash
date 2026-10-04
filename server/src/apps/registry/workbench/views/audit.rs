//! Native HTTP endpoints.
use crate::apps::registry::workbench::audit::AuditHistory;
use crate::apps::registry::workbench::serializers::audit::AuditQuery;
use crate::authorization::identity::Actor;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::get;
use reinhardt::http::ViewResult;

#[get(
	"/api/workbench/versions/{id}/{version}/audit",
	name = "workbench-version-audit",
	auth = "protected"
)]
pub async fn audit(
	#[inject] service: Depends<AuditHistory>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
	Query(query): Query<AuditQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.audit(actor, (id, version), query).await)
}
