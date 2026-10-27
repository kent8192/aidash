//! capabilities application module
//!
//! A RESTful API application

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;
/// A Preview gateway over `store` for integration tests outside this crate.
/// Production builds construct the same authority in `RuntimeTasks` or the
/// `preview-gateway` command.
#[doc(hidden)]
pub fn preview_gateway_for_test(
	store: crate::store::Store,
	domain: &str,
) -> std::sync::Arc<dyn aidash_application::preview::Authority> {
	std::sync::Arc::new(crate::apps::execution::repositories::previews::Gateway {
		store,
		domain: domain.to_owned(),
	})
}
