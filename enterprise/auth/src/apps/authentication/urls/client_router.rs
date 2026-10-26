// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! App-owned browser routes.
pub fn client_url_patterns() -> reinhardt::pages::router::ClientRouter {
	use reinhardt::pages::router::ClientRouter;
	ClientRouter::new()
		.route("start", "/", crate::app::page)
		.route("sign-in", "/sign-in", crate::app::page)
		.route("account", "/settings/account", crate::app::page)
}
