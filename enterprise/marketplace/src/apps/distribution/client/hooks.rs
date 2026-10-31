// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! App-owned asynchronous reads and browser navigation lifetimes.
use crate::api::ApiError;
use gloo_events::EventListener;
use reinhardt::pages::reactive::hooks::use_retained_effect;
use reinhardt::pages::server_fn::ServerFnError;
use reinhardt::pages::{Signal, deps, use_resource, use_state};
use serde::{Serialize, de::DeserializeOwned};
use std::future::Future;

pub type Resource<T> = reinhardt::pages::Resource<T, ApiError>;
pub use reinhardt::pages::ResourceState as Load;

/// A fresh read owned by the mounted route. The framework cancels stale
/// completions and refetch enters Loading, so a failed protected read cannot
/// retain a prior successful response.
pub fn use_service<T, F, Fut>(loader: F) -> Resource<T>
where
	T: Clone + Serialize + DeserializeOwned + 'static,
	F: Fn() -> Fut + 'static,
	Fut: Future<Output = Result<T, ServerFnError>> + 'static,
{
	use_resource(
		move || {
			let request = loader();
			async move { request.await.map_err(ApiError::from) }
		},
		deps![],
	)
}

fn current_route() -> String {
	web_sys::window()
		.and_then(|window| window.location().hash().ok())
		.map(|hash| hash.trim_start_matches('#').to_owned())
		.filter(|hash| !hash.is_empty())
		.unwrap_or_else(|| "/".into())
}

pub fn use_hash_route() -> Signal<String> {
	let (route, _) = use_state(current_route());
	use_retained_effect(
		move || {
			let listener = web_sys::window().map(|window| {
				let scroll_window = window.clone();
				EventListener::new(&window, "hashchange", move |_| {
					let _ = route.try_set(current_route());
					scroll_window.scroll_to_with_x_and_y(0.0, 0.0);
				})
			});
			Some(move || drop(listener))
		},
		deps![],
	);
	route
}
