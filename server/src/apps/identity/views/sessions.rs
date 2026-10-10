//! Native HTTP endpoints.
use crate::apps::identity::oidc::{BrowserOrigin, DashboardSessions};
use crate::apps::identity::serializers::oidc::{
	AdminIdentityPage, AdminMappingPage, Approval, BackchannelLogout, CallbackQuery, LoginQuery,
	MappingRevision, OperatorGrantInput,
};
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::di::params::Form;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};
use uuid::Uuid;

#[get("/auth/config", name = "sessions-configuration", auth = "public")]
pub async fn configuration(#[inject] service: Depends<DashboardSessions>) -> ViewResult<Response> {
	crate::http::json(service.configuration().await)
}

#[get("/auth/login", name = "sessions-login", auth = "public")]
pub async fn login(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Query(query): Query<LoginQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.login(request.headers, query).await)
}

#[get("/auth/callback", name = "sessions-callback", auth = "public")]
pub async fn callback(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Query(query): Query<CallbackQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.callback(request.headers, query).await)
}

#[get("/auth/session", name = "sessions-session-info", auth = "public")]
pub async fn session_info(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::response(service.session_info(request.headers).await)
}

#[get(
	"/auth/registration",
	name = "sessions-registration-status",
	auth = "public"
)]
pub async fn registration_status(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::response(service.registration_status(request.headers).await)
}

#[post(
	"/auth/registration",
	name = "sessions-registration-create",
	auth = "public"
)]
pub async fn registration_create(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::json(service.registration_create(request.headers).await)
}

#[get(
	"/api/dashboard/registrations",
	name = "sessions-admin-registrations",
	auth = "protected"
)]
pub async fn admin_registrations(
	#[inject] service: Depends<DashboardSessions>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_registrations().await)
}

#[get(
	"/api/dashboard/identities/{id}",
	name = "sessions-admin-identity",
	auth = "protected"
)]
pub async fn admin_identity(
	#[inject] service: Depends<DashboardSessions>,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_identity(id).await)
}

#[post(
	"/api/dashboard/identities/{id}/restore",
	name = "sessions-admin-restore-identity",
	auth = "protected"
)]
pub async fn admin_restore_identity(
	#[inject] service: Depends<DashboardSessions>,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::status(service.admin_restore_identity(id).await)
}

#[get(
	"/api/dashboard/identities",
	name = "sessions-admin-identities",
	auth = "protected"
)]
pub async fn admin_identities(
	#[inject] service: Depends<DashboardSessions>,
	Query(page): Query<AdminIdentityPage>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_identities(page).await)
}

#[get(
	"/api/dashboard/mappings",
	name = "sessions-admin-mappings",
	auth = "protected"
)]
pub async fn admin_mappings(
	#[inject] service: Depends<DashboardSessions>,
	Query(page): Query<AdminMappingPage>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_mappings(page).await)
}

#[post(
	"/api/dashboard/registrations/{id}/approve",
	name = "sessions-admin-approve",
	auth = "protected"
)]
pub async fn admin_approve(
	#[inject] service: Depends<DashboardSessions>,
	#[inject] actor: Option<BrowserOrigin>,
	Path(id): Path<Uuid>,
	Json(input): Json<Approval>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_approve(actor, id, input).await)
}

#[post(
	"/api/dashboard/registrations/{id}/reject",
	name = "sessions-admin-reject",
	auth = "protected"
)]
pub async fn admin_reject(
	#[inject] service: Depends<DashboardSessions>,
	#[inject] actor: Option<BrowserOrigin>,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_reject(actor, id).await)
}

#[post(
	"/api/dashboard/identities/{id}/operator-grant",
	name = "sessions-admin-operator-grant",
	auth = "protected"
)]
pub async fn admin_operator_grant(
	#[inject] service: Depends<DashboardSessions>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperatorGrantInput>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_operator_grant(id, input).await)
}

#[get(
	"/api/dashboard/operator-grants",
	name = "sessions-admin-operator-grants",
	auth = "protected"
)]
pub async fn admin_operator_grants(
	#[inject] service: Depends<DashboardSessions>,
) -> ViewResult<Response> {
	crate::http::json(service.admin_operator_grants().await)
}

#[post(
	"/api/dashboard/mappings/{id}/disable",
	name = "sessions-admin-disable-mapping",
	auth = "protected"
)]
pub async fn admin_disable_mapping(
	#[inject] service: Depends<DashboardSessions>,
	#[inject] actor: Option<BrowserOrigin>,
	Path(id): Path<Uuid>,
	Json(input): Json<MappingRevision>,
) -> ViewResult<Response> {
	crate::http::status(service.admin_disable_mapping(actor, id, input).await)
}

#[post("/auth/logout", name = "sessions-logout", auth = "public")]
pub async fn logout(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::response(service.logout(request.headers).await)
}

#[post("/auth/activity", name = "sessions-activity", auth = "public")]
pub async fn activity(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::status(service.activity(request.headers).await)
}

#[post("/auth/logout-all", name = "sessions-logout-all", auth = "public")]
pub async fn logout_all(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::response(service.logout_all(request.headers).await)
}

#[post(
	"/auth/backchannel-logout",
	name = "sessions-backchannel-logout",
	auth = "public"
)]
pub async fn backchannel_logout(
	#[inject] service: Depends<DashboardSessions>,
	Form(body): Form<BackchannelLogout>,
) -> ViewResult<Response> {
	crate::http::status(service.backchannel_logout(body).await)
}

#[get(
	"/auth/gcip/transaction",
	name = "sessions-gcip-transaction",
	auth = "public"
)]
pub async fn gcip_transaction(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Query(query): Query<crate::apps::identity::services::gcip::TransactionQuery>,
) -> ViewResult<Response> {
	crate::http::response(
		crate::apps::identity::services::gcip::configuration(
			&service.runtime,
			request.headers,
			query.state,
		)
		.await,
	)
}
#[post(
	"/auth/gcip/exchange",
	name = "sessions-gcip-exchange",
	auth = "public"
)]
pub async fn gcip_exchange(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Json(input): Json<crate::apps::identity::services::gcip::Exchange>,
) -> ViewResult<Response> {
	crate::http::response(
		crate::apps::identity::services::gcip::exchange(&service.runtime, request.headers, input)
			.await,
	)
}
