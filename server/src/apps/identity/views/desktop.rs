//! Reinhardt transport for the existing desktop broker protocol.
use crate::apps::identity::{
	repositories::desktop, serializers::desktop::*, services::oidc::DashboardSessions,
};
use crate::http::json::Json;
use reinhardt::di::params::Form;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Query, Request, Response, get, post};
#[post("/auth/desktop/start", name = "desktop-start", auth = "public")]
pub async fn start(
	#[inject] service: Depends<DashboardSessions>,
	Json(input): Json<Start>,
) -> ViewResult<Response> {
	crate::http::json(
		aidash_application::authorization::desktop::start(
			&crate::bootstrap::desktop_protocol(&service.runtime),
			input,
		)
		.await
		.map_err(crate::Error::from),
	)
}
#[get("/auth/desktop/authorize", name = "desktop-authorize", auth = "public")]
pub async fn authorize(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Query(input): Query<AuthorizationRequest>,
) -> ViewResult<Response> {
	crate::http::response(desktop::authorize(&service.runtime, request.headers, input).await)
}
#[post("/auth/desktop/authorize", name = "desktop-consent", auth = "public")]
pub async fn consent(
	#[inject] service: Depends<DashboardSessions>,
	request: Request,
	Form(input): Form<Consent>,
) -> ViewResult<Response> {
	crate::http::response(desktop::consent(&service.runtime, request.headers, input).await)
}
#[post("/auth/desktop/exchange", name = "desktop-exchange", auth = "public")]
pub async fn exchange(
	#[inject] service: Depends<DashboardSessions>,
	Json(input): Json<Exchange>,
) -> ViewResult<Response> {
	crate::http::json(
		aidash_application::authorization::desktop::exchange(
			&crate::bootstrap::desktop_protocol(&service.runtime),
			input,
		)
		.await
		.map_err(crate::Error::from),
	)
}
#[post("/auth/desktop/refresh", name = "desktop-refresh", auth = "public")]
pub async fn refresh(
	#[inject] service: Depends<DashboardSessions>,
	Json(input): Json<Renewal>,
) -> ViewResult<Response> {
	crate::http::json(
		aidash_application::authorization::desktop::refresh(
			&crate::bootstrap::desktop_protocol(&service.runtime),
			input,
		)
		.await
		.map_err(crate::Error::from),
	)
}
#[post("/auth/desktop/revoke", name = "desktop-revoke", auth = "public")]
pub async fn revoke(
	#[inject] service: Depends<DashboardSessions>,
	Json(input): Json<Revocation>,
) -> ViewResult<Response> {
	crate::http::status(
		aidash_application::authorization::desktop::revoke(
			&crate::bootstrap::desktop_protocol(&service.runtime),
			input,
		)
		.await
		.map(|()| reinhardt::StatusCode::NO_CONTENT)
		.map_err(crate::Error::from),
	)
}
