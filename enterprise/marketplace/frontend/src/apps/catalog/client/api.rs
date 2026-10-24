//! Same-origin Marketplace BFF transport. OAuth credentials never enter WASM.

use gloo_net::http::Request;
use reinhardt::pages::reactive::Signal;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{cell::Cell, rc::Rc};
use wasm_bindgen_futures::spawn_local;
use web_sys::RequestCredentials;

pub const API: &str = "/api/marketplace";

#[derive(Clone, Deserialize)]
pub struct Plugin {
	pub id: String,
	pub publisher_name: String,
	pub slug: String,
	pub name: String,
	pub description: String,
	pub license: String,
	pub origin: String,
	pub price_cents: u64,
	pub currency: String,
	pub declared_permissions: Vec<String>,
	pub latest_version: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct PluginVersion {
	pub version: String,
	pub digest: String,
	pub size_bytes: u64,
	pub recalled: bool,
	pub recall_reason: Option<String>,
	pub origin: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Session {
	pub authenticated: bool,
	pub csrf_token: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Publisher {
	pub id: String,
	pub name: String,
}

#[derive(Clone, Deserialize)]
pub struct LibraryEntry {
	pub plugin_id: String,
	pub slug: String,
	pub name: String,
	pub versions: Vec<PluginVersion>,
}

#[derive(Clone, Deserialize)]
pub struct Catalog {
	#[serde(default)]
	pub plugins: Vec<Plugin>,
}

#[derive(Clone, Deserialize)]
pub struct Detail {
	pub plugin: Plugin,
	pub versions: Vec<PluginVersion>,
}

#[derive(Clone, Deserialize)]
pub struct Library {
	pub entries: Vec<LibraryEntry>,
}

#[derive(Clone, Deserialize)]
pub struct Publishers {
	pub publishers: Vec<Publisher>,
}

#[derive(Deserialize)]
pub struct CreatedPublisher {
	pub publisher: Publisher,
}

#[derive(Deserialize)]
pub struct CreatedPlugin {
	pub plugin: Plugin,
}

#[derive(Deserialize)]
pub struct PublishedVersion {
	pub version: PluginVersion,
}

#[derive(Clone)]
pub struct ApiError {
	pub status: Option<u16>,
	pub message: String,
}

impl ApiError {
	pub fn local(message: impl Into<String>) -> Self {
		Self {
			status: None,
			message: message.into(),
		}
	}
}

async fn response<T: DeserializeOwned>(response: gloo_net::http::Response) -> Result<T, ApiError> {
	let status = response.status();
	let body = response
		.text()
		.await
		.map_err(|_| ApiError::local("Unable to read the server response. Please try again."))?;
	if !response.ok() {
		let message = serde_json::from_str::<Value>(&body)
			.ok()
			.and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned))
			.unwrap_or_else(|| format!("Request failed ({status}). Please try again."));
		return Err(ApiError {
			status: Some(status),
			message,
		});
	}
	serde_json::from_str(if status == 204 { "null" } else { &body }).map_err(|_| {
		ApiError::local("The server returned an unreadable response. Please try again.")
	})
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
	let result = Request::get(path)
		.credentials(RequestCredentials::Include)
		.send()
		.await
		.map_err(|_| {
			ApiError::local("Unable to reach Marketplace. Check your connection and try again.")
		})?;
	response(result).await
}

pub async fn write<T: DeserializeOwned>(
	path: &str,
	body: &Value,
	session: &Session,
) -> Result<T, ApiError> {
	let Some(csrf) = session
		.csrf_token
		.as_ref()
		.filter(|_| session.authenticated)
	else {
		return Err(ApiError {
			status: Some(401),
			message: "Sign in again before making this change.".into(),
		});
	};
	let request = Request::post(path)
		.credentials(RequestCredentials::Include)
		.header("Content-Type", "application/json")
		.header("X-Marketplace-CSRF", csrf)
		.body(body.to_string())
		.map_err(|_| ApiError::local("Unable to prepare this request."))?;
	let result = request.send().await.map_err(|_| {
		ApiError::local("Unable to reach Marketplace. Check your connection and try again.")
	})?;
	response(result).await
}

#[derive(Clone)]
pub enum Load<T> {
	Pending,
	Ready(T),
	Failed(ApiError),
}

/// A page-owned read. Revisiting a protected route creates a new read instead
/// of displaying a previous route's successful response after an outage.
/// Late responses are discarded after the mounted Pages owner is disposed.
#[derive(Clone)]
pub struct Resource<T: Clone + 'static> {
	pub state: Signal<Load<T>>,
	path: String,
	generation: Rc<Cell<u64>>,
}

impl<T: Clone + DeserializeOwned + 'static> Resource<T> {
	pub fn new(path: impl Into<String>) -> Self {
		let resource = Self {
			state: Signal::new(Load::Pending),
			path: path.into(),
			generation: Rc::new(Cell::new(0)),
		};
		resource.reload();
		resource
	}

	pub fn reload(&self) {
		let resource = self.clone();
		let generation = resource.generation.get().wrapping_add(1);
		resource.generation.set(generation);
		if resource.state.try_set(Load::Pending).is_err() {
			return;
		}
		spawn_local(async move {
			let result = get(&resource.path).await;
			if resource.generation.get() == generation {
				let _ = resource.state.try_set(match result {
					Ok(value) => Load::Ready(value),
					Err(error) => Load::Failed(error),
				});
			}
		});
	}
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
