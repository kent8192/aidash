//! The two existing validation boundaries use a safe JSON error envelope.
use async_trait::async_trait;
use reinhardt::Request;
use reinhardt::di::params::{FromRequest, HasInner, ParamContext, ParamResult};
use serde::de::DeserializeOwned;
use std::ops::Deref;

/// JSON extraction for Workspace and Conversation creation. Views run the
/// existing field validation after extraction without echoing submitted values.
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
	type Target = T;
	fn deref(&self) -> &T {
		&self.0
	}
}

impl<T> HasInner for Json<T> {
	type Inner = T;
	fn inner_ref(&self) -> &T {
		&self.0
	}
	fn into_inner(self) -> T {
		self.0
	}
}

#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Json<T> {
	async fn from_request(request: &Request, context: &ParamContext) -> ParamResult<Self> {
		super::json::extract(request, context, true).await.map(Self)
	}
}
