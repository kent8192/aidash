//! Authentication app HTTP protocol entry point.
use super::{http::Http, service::Authentication};
use std::sync::Arc;
pub fn handler(auth: Arc<Authentication>) -> Http {
	Http::new(auth)
}
