#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(all(feature = "e2e", not(debug_assertions)))]
compile_error!("The e2e WebDriver feature must never be enabled in release builds");
mod auth;
mod credentials;
#[cfg(feature = "e2e")]
mod e2e;
mod profiles;

use auth::{Access, Cached, Endpoint, Failure, Tokens};
use credentials::Credential;
use profiles::{Profile, Profiles, Settings};
use serde_json::json;
use std::{
	collections::HashMap,
	sync::atomic::{AtomicU64, Ordering},
	time::Instant,
};
use tauri::{Manager, State};
#[cfg(not(feature = "e2e"))]
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{Mutex, Notify};
use zeroize::Zeroizing;

pub struct DesktopState {
	profiles: Mutex<Profiles>,
	// Serialize all credential reads, rotations and writes across IPC callers.
	operation: Mutex<HashMap<String, Cached>>,
	generation: AtomicU64,
	cancel: Notify,
	client: reqwest::Client,
}
impl DesktopState {
	fn current(&self, generation: u64) -> Result<(), String> {
		if self.generation.load(Ordering::SeqCst) != generation {
			Err("Connection changed; operation cancelled".into())
		} else {
			Ok(())
		}
	}
	fn invalidate(&self) {
		self.generation.fetch_add(1, Ordering::SeqCst);
		self.cancel.notify_waiters();
	}
}
#[tauri::command]
async fn connection_settings(state: State<'_, DesktopState>) -> Result<Settings, String> {
	Ok(state.profiles.lock().await.settings.clone())
}
#[tauri::command]
async fn save_connection(
	state: State<'_, DesktopState>,
	name: String,
	origin: String,
) -> Result<Profile, String> {
	let origin = profiles::origin(&origin)?;
	let name = name.trim();
	if name.is_empty() || name.len() > 240 || name.chars().any(char::is_control) {
		return Err("Enter a connection name of at most 80 characters".into());
	}
	let mut profiles = state.profiles.lock().await;
	let mut settings = profiles.settings.clone();
	let profile = if let Some(profile) = settings.profiles.iter_mut().find(|p| p.origin == origin) {
		profile.name = name.into();
		profile.clone()
	} else {
		if settings.profiles.len() >= 32 {
			return Err("Remove a saved connection before adding another (limit 32)".into());
		}
		let profile = Profile {
			id: uuid::Uuid::new_v4().to_string(),
			name: name.into(),
			origin,
		};
		settings.profiles.push(profile.clone());
		profile
	};
	profiles.persist(settings)?;
	Ok(profile)
}
#[tauri::command]
async fn select_connection(
	state: State<'_, DesktopState>,
	id: Option<String>,
) -> Result<(), String> {
	state.invalidate();
	let mut cache = state.operation.lock().await;
	cache.clear();
	let mut profiles = state.profiles.lock().await;
	let mut settings = profiles.settings.clone();
	if id
		.as_ref()
		.is_some_and(|id| !settings.profiles.iter().any(|p| p.id == *id))
	{
		return Err("Unknown connection".into());
	}
	settings.selected = id;
	profiles.persist(settings)
}
async fn revoke(state: &DesktopState, profile: &Profile) -> Result<(), String> {
	if let Some(credential) = credentials::read(profile.clone()).await? {
		let _: serde_json::Value = auth::request(
			&state.client,
			profile,
			Endpoint::Revoke,
			Some(json!({"refresh_token":credential.current})),
		)
		.await
		.map_err(Failure::message)?;
		credentials::delete(profile.clone()).await?;
	}
	Ok(())
}
#[tauri::command]
async fn remove_connection(state: State<'_, DesktopState>, id: String) -> Result<(), String> {
	state.invalidate();
	let mut cache = state.operation.lock().await;
	let profile = state
		.profiles
		.lock()
		.await
		.settings
		.profiles
		.iter()
		.find(|p| p.id == id)
		.cloned()
		.ok_or("Unknown connection")?;
	// Keep the connection recoverable if the server or OS store is unavailable.
	revoke(&state, &profile).await?;
	cache.remove(&id);
	let mut profiles = state.profiles.lock().await;
	let mut settings = profiles.settings.clone();
	settings.profiles.retain(|p| p.id != id);
	if settings.selected.as_ref() == Some(&id) {
		settings.selected = None;
	}
	profiles.persist(settings)
}
#[tauri::command]
async fn desktop_access(
	state: State<'_, DesktopState>,
	rejected_token: Option<String>,
) -> Result<Option<Access>, String> {
	let generation = state.generation.load(Ordering::SeqCst);
	let mut cache = state.operation.lock().await;
	state.current(generation)?;
	let profile = state.profiles.lock().await.selected()?;
	if let Some(cached) = cache.get(&profile.id)
		&& cached.expires > Instant::now()
		&& rejected_token
			.as_ref()
			.is_none_or(|rejected| rejected != cached.token.as_str())
	{
		return Ok(Some(cached.access()));
	}
	let Some(mut credential) = credentials::read(profile.clone()).await? else {
		cache.remove(&profile.id);
		return Ok(None);
	};
	let next = Zeroizing::new(
		credential
			.pending
			.clone()
			.unwrap_or_else(|| format!("aidash_refresh_{}", auth::secret())),
	);
	// Write-ahead journal: a lost response is retried with the identical pair.
	if credential.pending.is_none() {
		credential.pending = Some(next.to_string());
		credentials::write(
			profile.clone(),
			Credential {
				current: credential.current.clone(),
				pending: credential.pending.clone(),
			},
		)
		.await?;
	}
	state.current(generation)?;
	let result = auth::request::<Tokens>(
		&state.client,
		&profile,
		Endpoint::Refresh,
		Some(json!({"refresh_token":credential.current,"next_token":next.as_str()})),
	)
	.await;
	let tokens = match result {
		Ok(tokens) => tokens,
		Err(Failure::Unauthorized) => {
			credentials::delete(profile.clone()).await?;
			cache.remove(&profile.id);
			return Ok(None);
		}
		Err(error) => return Err(error.message()),
	};
	if tokens.refresh_token != *next
		|| !tokens.access_token.starts_with("aidash_desktop_")
		|| !(1..=900).contains(&tokens.expires_in)
	{
		return Err(
			"Invalid authentication response; saved recovery credentials have been preserved"
				.into(),
		);
	}
	credentials::write(
		profile.clone(),
		Credential {
			current: tokens.refresh_token.clone(),
			pending: None,
		},
	)
	.await?;
	state.current(generation)?;
	let cached = Cached::from_tokens(&tokens);
	let access = cached.access();
	cache.insert(profile.id, cached);
	Ok(Some(access))
}
#[tauri::command]
async fn desktop_login(
	_app: tauri::AppHandle,
	state: State<'_, DesktopState>,
) -> Result<(), String> {
	let generation = state.generation.load(Ordering::SeqCst);
	let mut cache = state.operation.lock().await;
	state.current(generation)?;
	let profile = state.profiles.lock().await.selected()?;
	let previous = credentials::read(profile.clone()).await?;
	let config: auth::Config = auth::request(&state.client, &profile, Endpoint::Config, None)
		.await
		.map_err(Failure::message)?;
	if config.desktop_protocol != Some(1) {
		return Err(
			"This Aidash server does not support desktop sign-in. Upgrade the server.".into(),
		);
	}
	if !config.enabled {
		return Err("Configure Google sign-in on the Aidash server first.".into());
	}
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.map_err(|_| "Cannot bind the local sign-in callback")?;
	let redirect_uri = format!(
		"http://{}/callback",
		listener
			.local_addr()
			.map_err(|_| "Cannot read callback address")?
	);
	let verifier = Zeroizing::new(auth::secret());
	let csrf = Zeroizing::new(auth::secret());
	let started:auth::Started=auth::request(&state.client,&profile,Endpoint::Start,Some(json!({"redirect_uri":redirect_uri,"state":csrf.as_str(),"code_challenge":auth::challenge(&verifier)}))).await.map_err(Failure::message)?;
	state.current(generation)?;
	let url = auth::authorization_url(&started.authorization_url, &profile)?;
	#[cfg(feature = "e2e")]
	let _fixture_browser = e2e::open_browser(&url)?;
	#[cfg(not(feature = "e2e"))]
	_app.opener()
		.open_url(url.as_str(), None::<&str>)
		.map_err(|_| "Cannot open the system browser")?;
	let code = Zeroizing::new(auth::receive(listener, &state, generation, &csrf).await?);
	state.current(generation)?;
	let tokens:Tokens=auth::request(&state.client,&profile,Endpoint::Exchange,Some(json!({"code":code.as_str(),"state":csrf.as_str(),"verifier":verifier.as_str(),"redirect_uri":redirect_uri}))).await.map_err(Failure::message)?;
	if !tokens.access_token.starts_with("aidash_desktop_")
		|| !tokens.refresh_token.starts_with("aidash_refresh_")
		|| !(1..=900).contains(&tokens.expires_in)
	{
		return Err("Invalid desktop sign-in response".into());
	}
	let save = async {
		state.current(generation)?;
		credentials::write(
			profile.clone(),
			Credential {
				current: tokens.refresh_token.clone(),
				pending: None,
			},
		)
		.await
	}
	.await;
	if let Err(error) = save {
		let _ = auth::request::<serde_json::Value>(
			&state.client,
			&profile,
			Endpoint::Revoke,
			Some(json!({"refresh_token":tokens.refresh_token})),
		)
		.await;
		return Err(error);
	}
	if let Some(previous) = previous {
		let _ = auth::request::<serde_json::Value>(
			&state.client,
			&profile,
			Endpoint::Revoke,
			Some(json!({"refresh_token":previous.current})),
		)
		.await;
	}
	state.current(generation)?;
	cache.insert(profile.id, Cached::from_tokens(&tokens));
	Ok(())
}
#[tauri::command]
async fn desktop_logout(state: State<'_, DesktopState>) -> Result<(), String> {
	state.invalidate();
	let mut cache = state.operation.lock().await;
	let profile = state.profiles.lock().await.selected()?;
	revoke(&state, &profile).await?;
	cache.remove(&profile.id);
	Ok(())
}
fn main() {
	#[cfg(feature = "e2e")]
	if e2e::prepare().expect("prepare isolated desktop test") {
		return;
	}
	let builder = tauri::Builder::default();
	#[cfg(feature = "e2e")]
	let builder = builder
		.plugin(tauri_plugin_wdio::init())
		.plugin(tauri_plugin_wdio_webdriver::init());
	builder
		.plugin(tauri_plugin_single_instance::init(|app, _, _| {
			if let Some(window) = app.get_webview_window("main") {
				let _ = window.show();
				let _ = window.set_focus();
			}
		}))
		.plugin(tauri_plugin_opener::init())
		.setup(|app| {
			#[cfg(feature = "e2e")]
			let path = e2e::profiles_path()?;
			#[cfg(not(feature = "e2e"))]
			let path = app
				.path()
				.app_config_dir()?
				.join(if cfg!(debug_assertions) {
					"connections-development.json"
				} else {
					"connections.json"
				});
			let profiles = Profiles::load(path).map_err(std::io::Error::other)?;
			app.manage(DesktopState {
				profiles: Mutex::new(profiles),
				operation: Mutex::new(HashMap::new()),
				generation: AtomicU64::new(0),
				cancel: Notify::new(),
				client: auth::client().map_err(std::io::Error::other)?,
			});
			let window =
				tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
					.on_navigation(profiles::local_navigation)
					.on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
			#[cfg(feature = "e2e")]
			let window = window.initialization_script(include_str!(concat!(
				env!("CARGO_MANIFEST_DIR"),
				"/gen/wdio.js"
			)));
			window.build()?;
			Ok(())
		})
		.invoke_handler(tauri::generate_handler![
			connection_settings,
			save_connection,
			select_connection,
			remove_connection,
			desktop_login,
			desktop_access,
			desktop_logout
		])
		.run(tauri::generate_context!())
		.expect("run Aidash Desktop");
}
