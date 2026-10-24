//! Compose the owning authentication app without Cloud routing or middleware.
use crate::{http::Http, service::Authentication};
use std::sync::Arc;
pub fn handler(auth: Arc<Authentication>) -> Http {
	crate::apps::authentication::urls::handler(auth)
}
