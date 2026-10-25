//! Pages resources backed by the app's generated typed server functions.
use super::super::super::serializers::*;
pub(crate) use super::super::super::serializers::{
	Catalog, CreatedPlugin, CreatedPublisher, Detail, Library, Plugin, PluginDraft, PluginVersion,
	Publication, PublishedVersion, Publisher, Publishers, Session,
};
use super::super::super::server_fn::*;
use reinhardt::pages::server_fn::ServerFnError;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct ApiError {
	pub status: Option<u16>,
	pub message: String,
}

impl std::fmt::Display for ApiError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.message)
	}
}

impl ApiError {
	pub fn local(message: impl Into<String>) -> Self {
		Self {
			status: None,
			message: message.into(),
		}
	}
}

impl From<ServerFnError> for ApiError {
	fn from(error: ServerFnError) -> Self {
		Self {
			status: error.status(),
			message: error.user_message().to_owned(),
		}
	}
}
fn csrf(session: &Session) -> Result<String, ApiError> {
	session
		.csrf_token
		.as_ref()
		.filter(|_| session.authenticated)
		.cloned()
		.ok_or_else(|| ApiError {
			status: Some(401),
			message: "Sign in again before making this change.".into(),
		})
}
pub async fn logout(session: &Session) -> Result<Session, ApiError> {
	marketplace_logout(csrf(session)?).await.map_err(Into::into)
}
pub async fn acquire(id: String, session: &Session) -> Result<Acquisition, ApiError> {
	marketplace_acquire(id, csrf(session)?)
		.await
		.map_err(Into::into)
}
pub async fn create_publisher(
	name: String,
	session: &Session,
) -> Result<CreatedPublisher, ApiError> {
	marketplace_create_publisher(name, csrf(session)?)
		.await
		.map_err(Into::into)
}
pub async fn create_plugin(
	draft: PluginDraft,
	session: &Session,
) -> Result<CreatedPlugin, ApiError> {
	marketplace_create_plugin(draft, csrf(session)?)
		.await
		.map_err(Into::into)
}
pub async fn publish(
	id: String,
	publication: Publication,
	session: &Session,
) -> Result<PublishedVersion, ApiError> {
	marketplace_publish(id, publication, csrf(session)?)
		.await
		.map_err(Into::into)
}
pub fn encode(value: &str) -> String {
	js_sys::encode_uri_component(value)
		.as_string()
		.unwrap_or_default()
}

pub fn price(plugin: &Plugin) -> String {
	if plugin.price_cents == 0 {
		return "Free".into();
	}
	let amount = format!(
		"{}.{:02}",
		plugin.price_cents / 100,
		plugin.price_cents % 100
	);
	match plugin.currency.as_str() {
		"USD" => format!("${amount}"),
		_ => format!("{amount} {}", plugin.currency),
	}
}
