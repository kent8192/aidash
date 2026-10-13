//! Provider Authorization endpoints: a protected start and the public callback
//! the provider redirects the browser to.
use super::super::{
	serializers::provider_authorizations::{CallbackQuery, Start},
	services::{oidc::BrowserOrigin, provider_authorizations::ProviderAuthorizations},
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Query, Request, Response, get, http::ViewResult, post};

#[post(
	"/api/tenants/{tenant}/provider-authorizations/{provider}",
	name = "provider-authorization-start",
	auth = "protected"
)]
pub async fn start(
	#[inject] service: Depends<ProviderAuthorizations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	request: Request,
	Path((tenant, provider)): Path<(String, String)>,
	Json(input): Json<Start>,
) -> ViewResult<Response> {
	crate::http::response(
		service
			.start(actor, origin, request.headers, tenant, provider, input)
			.await,
	)
}

#[get(
	"/auth/provider-authorizations/{provider}/callback",
	name = "provider-authorization-callback",
	auth = "public"
)]
pub async fn callback(
	#[inject] service: Depends<ProviderAuthorizations>,
	request: Request,
	Path(provider): Path<String>,
	Query(query): Query<CallbackQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.callback(request.headers, provider, query).await)
}
