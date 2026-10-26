// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn marketplace_session(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Session, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			"/api/marketplace/session",
			None,
			None,
		)
		.await
}

#[server_fn]
pub async fn marketplace_catalog(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Catalog, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			"/api/marketplace/catalog",
			None,
			None,
		)
		.await
}

#[server_fn]
pub async fn marketplace_detail(
	slug: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Detail, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			&(format!(
				"/api/marketplace/catalog/{}",
				percent_encoding::utf8_percent_encode(&slug, percent_encoding::NON_ALPHANUMERIC)
			)),
			None,
			None,
		)
		.await
}
