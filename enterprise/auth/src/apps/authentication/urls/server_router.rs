// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Explicit native registration of the app's typed server functions.
use super::super::server_fn::*;
use reinhardt::{ServerRouter, pages::server_fn::ServerFnRouterExt};
pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.server_fn(auth_login_context::marker)
		.server_fn(auth_select_tenant::marker)
		.server_fn(auth_verify::marker)
		.server_fn(auth_choose::marker)
		.server_fn(auth_account::marker)
		.server_fn(auth_link::marker)
		.server_fn(auth_unlink::marker)
		.server_fn(auth_logout_all::marker)
}
