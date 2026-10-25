//! One-use loopback relay. The Aidash server owns OAuth state, PKCE and tokens;
//! this module only checks the server-issued browser URL and relays its callback.
use reqwest::Url;
use serde::Serialize;
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
	sync::watch,
	time::Instant,
};
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(300);
const CANCELLED: &str = "Plan authorization cancelled";
const TIMED_OUT: &str = "Plan authorization timed out. Please retry.";

#[derive(Serialize)]
pub struct Listening {
	pub id: String,
	pub port: u16,
}

struct Entry {
	listener: Option<TcpListener>,
	port: u16,
	deadline: Instant,
	cancel: watch::Sender<bool>,
}

#[derive(Clone, Default)]
pub struct Relay {
	entries: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Relay {
	pub async fn listen(&self) -> Result<Listening, String> {
		let listener = TcpListener::bind("127.0.0.1:0")
			.await
			.map_err(|_| "Cannot bind the Plan callback listener")?;
		let port = listener
			.local_addr()
			.map_err(|_| "Cannot read Plan callback address")?
			.port();
		let id = uuid::Uuid::new_v4().to_string();
		let deadline = Instant::now() + LIFETIME;
		let (cancel, mut expiration_cancel) = watch::channel(false);
		{
			let mut entries = self.entries.lock().unwrap();
			if entries.len() >= 4 {
				return Err("Cancel an existing Plan authorization before starting another".into());
			}
			entries.insert(
				id.clone(),
				Entry {
					listener: Some(listener),
					port,
					deadline,
					cancel,
				},
			);
		}
		// Expire unclaimed listeners too; no browser wait is needed to release a port.
		let entries = Arc::downgrade(&self.entries);
		let expired_id = id.clone();
		tokio::spawn(async move {
			tokio::select! {
				_ = expiration_cancel.changed() => {},
				_ = tokio::time::sleep_until(deadline) => {
					if let Some(entries) = entries.upgrade()
						&& let Some(entry) = entries.lock().unwrap().remove(&expired_id)
					{
						entry.cancel.send_replace(true);
					}
				},
			}
		});
		Ok(Listening { id, port })
	}

	/// Validation and one-use claiming happen before the caller opens a browser.
	pub fn prepare(&self, id: &str, authorization_url: &str) -> Result<Waiting, String> {
		let mut entries = self.entries.lock().unwrap();
		let entry = entries.get_mut(id).ok_or(CANCELLED)?;
		if entry.deadline <= Instant::now() {
			return Err(TIMED_OUT.into());
		}
		let authorization = Authorization::parse(authorization_url, entry.port)?;
		let listener = entry
			.listener
			.take()
			.ok_or("Plan authorization already waiting")?;
		Ok(Waiting {
			relay: self.clone(),
			id: id.into(),
			listener,
			authorization,
			deadline: entry.deadline,
			cancel: entry.cancel.subscribe(),
		})
	}

	pub fn cancel(&self, id: &str) {
		if let Some(entry) = self.entries.lock().unwrap().remove(id) {
			entry.cancel.send_replace(true);
		}
	}

	pub fn cancel_all(&self) {
		for (_, entry) in self.entries.lock().unwrap().drain() {
			entry.cancel.send_replace(true);
		}
	}
}

struct Authorization {
	url: Url,
	redirect: String,
	state: String,
	client: String,
}

fn parameters(url: &Url) -> Option<HashMap<String, String>> {
	let mut result = HashMap::new();
	for (name, value) in url.query_pairs() {
		if result
			.insert(name.into_owned(), value.into_owned())
			.is_some()
		{
			return None;
		}
	}
	Some(result)
}

fn bounded(value: &str, max: usize) -> bool {
	!value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn client_id(value: &str) -> bool {
	bounded(value, 256)
		&& value
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

impl Authorization {
	fn parse(value: &str, port: u16) -> Result<Self, String> {
		let invalid = || "Invalid OpenAI Plan authorization URL for this listener".to_string();
		if value.len() > 16384 || value.chars().any(char::is_control) {
			return Err(invalid());
		}
		let url = Url::parse(value).map_err(|_| invalid())?;
		if url.origin().ascii_serialization() != "https://auth.openai.com"
			|| url.path() != "/api/accounts/authorize"
			|| !url.username().is_empty()
			|| url.password().is_some()
			|| url.fragment().is_some()
		{
			return Err(invalid());
		}
		let parameters = parameters(&url).ok_or_else(invalid)?;
		let redirect = format!("http://127.0.0.1:{port}/auth/callback");
		let state = parameters
			.get("state")
			.filter(|v| bounded(v, 512))
			.ok_or_else(invalid)?
			.clone();
		let client = parameters
			.get("client_id")
			.filter(|v| client_id(v))
			.ok_or_else(invalid)?
			.clone();
		if parameters.get("redirect_uri") != Some(&redirect)
			|| parameters.get("response_type").map(String::as_str) != Some("code")
		{
			return Err(invalid());
		}
		Ok(Self {
			url,
			redirect,
			state,
			client,
		})
	}

	fn callback(&self, request: &str, address: &str) -> Option<String> {
		let mut lines = request.split("\r\n");
		let mut first = lines.next()?.split_whitespace();
		if first.next()? != "GET" {
			return None;
		}
		let target = first.next()?;
		if !target.starts_with("/auth/callback?")
			|| first.next()? != "HTTP/1.1"
			|| first.next().is_some()
		{
			return None;
		}
		let hosts = lines
			.take_while(|line| !line.is_empty())
			.filter_map(|line| line.split_once(':'))
			.filter(|(key, _)| key.eq_ignore_ascii_case("host"))
			.map(|(_, value)| value.trim())
			.collect::<Vec<_>>();
		if hosts != [address] {
			return None;
		}
		let url = Url::parse(&format!("http://{address}{target}")).ok()?;
		if url.fragment().is_some() || url.path() != "/auth/callback" {
			return None;
		}
		let parameters = parameters(&url)?;
		if parameters.get("state") != Some(&self.state) {
			return None;
		}
		if let Some(error) = parameters.get("error") {
			if !bounded(error, 256)
				|| parameters.keys().any(|key| {
					!["state", "error", "error_description", "error_uri"].contains(&key.as_str())
				}) {
				return None;
			}
		} else {
			if !parameters.get("code").is_some_and(|v| bounded(v, 4096))
				|| parameters
					.keys()
					.any(|key| !["state", "code", "client_id"].contains(&key.as_str()))
			{
				return None;
			}
			match parameters.get("client_id") {
				Some(client)
					if client_id(client)
						&& client != "dynamic_agent_client"
						&& (self.client == "dynamic_agent_client" || client == &self.client) => {}
				None if self.client != "dynamic_agent_client" => {}
				_ => return None,
			}
		}
		Some(format!("{}?{}", self.redirect, url.query()?))
	}
}

pub struct Waiting {
	relay: Relay,
	id: String,
	listener: TcpListener,
	authorization: Authorization,
	deadline: Instant,
	cancel: watch::Receiver<bool>,
}

impl Drop for Waiting {
	fn drop(&mut self) {
		self.relay.cancel(&self.id);
	}
}

impl Waiting {
	pub fn authorization_url(&self) -> &Url {
		&self.authorization.url
	}

	pub async fn receive(mut self) -> Result<String, String> {
		if Instant::now() >= self.deadline {
			return Err(TIMED_OUT.into());
		}
		if *self.cancel.borrow() {
			return Err(CANCELLED.into());
		}
		let result = tokio::select! {
			biased;
			_ = self.cancel.changed() => Err(CANCELLED.into()),
			result = tokio::time::timeout_at(self.deadline, receive(&self.listener, &self.authorization)) => result.map_err(|_| TIMED_OUT.to_string())?,
		};
		if *self.cancel.borrow() {
			return Err(CANCELLED.into());
		}
		result
	}
}

async fn receive(listener: &TcpListener, authorization: &Authorization) -> Result<String, String> {
	let address = listener
		.local_addr()
		.map_err(|_| "Cannot read Plan callback address")?
		.to_string();
	loop {
		let (mut socket, _) = listener
			.accept()
			.await
			.map_err(|_| "Cannot receive Plan callback")?;
		let exchange = async {
			let mut data = Zeroizing::new(Vec::new());
			let mut chunk = [0; 1024];
			loop {
				let count = socket.read(&mut chunk).await.ok()?;
				if count == 0 || data.len() + count > 8192 {
					return None;
				}
				data.extend_from_slice(&chunk[..count]);
				if data.windows(4).any(|value| value == b"\r\n\r\n") {
					break;
				}
			}
			let callback = std::str::from_utf8(&data)
				.ok()
				.and_then(|request| authorization.callback(request, &address));
			let (status, body) = if callback.is_some() {
				(
					"200 OK",
					"Authorization received. Return to Aidash Desktop.",
				)
			} else {
				("400 Bad Request", "Invalid Plan callback.")
			};
			let response = format!(
				"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
				body.len()
			);
			// The single-use callback is received even if the browser closes early.
			let _ = socket.write_all(response.as_bytes()).await;
			callback
		};
		if let Ok(Some(callback)) = tokio::time::timeout(Duration::from_secs(5), exchange).await {
			return Ok(callback);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	fn authorization(port: u16, client: &str) -> String {
		format!(
			"https://auth.openai.com/api/accounts/authorize?response_type=code&client_id={client}&state=server-state&redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Fauth%2Fcallback"
		)
	}

	async fn request(port: u16, target: &str, host: &str) -> String {
		let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
			.await
			.unwrap();
		socket
			.write_all(format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
			.await
			.unwrap();
		let mut response = String::new();
		socket.read_to_string(&mut response).await.unwrap();
		response
	}

	#[tokio::test]
	async fn one_use_callback_relays_issued_client_and_denial_without_tokens() {
		for query in [
			"code=one-use-code&state=server-state&client_id=oaiapp_issued",
			"error=access_denied&state=server-state",
		] {
			let relay = Relay::default();
			let allocated = relay.listen().await.unwrap();
			let waiting = relay
				.prepare(
					&allocated.id,
					&authorization(allocated.port, "dynamic_agent_client"),
				)
				.unwrap();
			assert!(
				relay
					.prepare(
						&allocated.id,
						&authorization(allocated.port, "dynamic_agent_client")
					)
					.is_err()
			);
			let path = format!("/auth/callback?{query}");
			let host = format!("127.0.0.1:{}", allocated.port);
			let (callback, response) =
				tokio::join!(waiting.receive(), request(allocated.port, &path, &host));
			assert_eq!(
				callback.unwrap(),
				format!("http://127.0.0.1:{}{path}", allocated.port)
			);
			assert!(response.starts_with("HTTP/1.1 200 OK"));
			assert!(!response.contains(query));
			assert!(
				tokio::net::TcpStream::connect(("127.0.0.1", allocated.port))
					.await
					.is_err()
			);
			assert!(
				relay
					.prepare(
						&allocated.id,
						&authorization(allocated.port, "dynamic_agent_client")
					)
					.is_err()
			);
		}
	}

	#[tokio::test]
	async fn invalid_callbacks_do_not_consume_the_listener() {
		for invalid in [
			"/callback?code=c&state=server-state&client_id=oaiapp_issued",
			"/auth/callback?code=c&state=wrong&client_id=oaiapp_issued",
			"/auth/callback?code=c&client_id=oaiapp_issued",
			"/auth/callback?code=c&state=server-state",
			"/auth/callback?code=c&state=server-state&client_id=",
			"/auth/callback?code=c&state=server-state&client_id=bad%2Fclient",
			"/auth/callback?state=server-state&client_id=oaiapp_issued",
			"/auth/callback?code=&state=server-state&client_id=oaiapp_issued",
			"/auth/callback?code=c&state=server-state&state=server-state&client_id=oaiapp_issued",
			"/auth/callback?code=c&state=server-state&client_id=oaiapp_issued&access_token=forbidden",
			"/auth/callback?error=access_denied&code=c&state=server-state",
		] {
			let relay = Relay::default();
			let allocated = relay.listen().await.unwrap();
			let waiting = relay
				.prepare(
					&allocated.id,
					&authorization(allocated.port, "dynamic_agent_client"),
				)
				.unwrap();
			let host = format!("127.0.0.1:{}", allocated.port);
			let (callback, _) = tokio::join!(waiting.receive(), async {
				assert!(
					request(allocated.port, invalid, &host)
						.await
						.starts_with("HTTP/1.1 400"),
					"{invalid}"
				);
				assert!(
					request(
						allocated.port,
						"/auth/callback?code=c&state=server-state&client_id=oaiapp_issued",
						"attacker.example"
					)
					.await
					.starts_with("HTTP/1.1 400")
				);
				request(
					allocated.port,
					"/auth/callback?code=c&state=server-state&client_id=oaiapp_issued",
					&host,
				)
				.await
			});
			assert!(callback.is_ok(), "{invalid}");
		}
	}

	#[tokio::test]
	async fn reconnect_allows_omitted_client_but_rejects_a_different_client() {
		let relay = Relay::default();
		let allocated = relay.listen().await.unwrap();
		let waiting = relay
			.prepare(
				&allocated.id,
				&authorization(allocated.port, "oaiapp_saved"),
			)
			.unwrap();
		let host = format!("127.0.0.1:{}", allocated.port);
		let (callback, _) = tokio::join!(waiting.receive(), async {
			assert!(
				request(
					allocated.port,
					"/auth/callback?code=c&state=server-state&client_id=oaiapp_other",
					&host
				)
				.await
				.starts_with("HTTP/1.1 400")
			);
			request(
				allocated.port,
				"/auth/callback?code=c&state=server-state",
				&host,
			)
			.await
		});
		assert!(callback.unwrap().ends_with("code=c&state=server-state"));
	}

	#[tokio::test]
	async fn authorization_must_be_openai_and_name_this_exact_listener() {
		let relay = Relay::default();
		let allocated = relay.listen().await.unwrap();
		let valid = authorization(allocated.port, "dynamic_agent_client");
		for invalid in [
			valid.replace("https:", "http:"),
			valid.replace("auth.openai.com", "attacker.example"),
			valid.replace("auth.openai.com", "auth.openai.com.attacker.example"),
			valid.replace("auth.openai.com", "user@auth.openai.com"),
			valid.replace("auth.openai.com", "auth.openai.com:444"),
			valid.replace("/api/accounts/authorize", "/logout"),
			valid.replace("127.0.0.1", "localhost"),
			valid.replace("%2Fauth%2Fcallback", "%2Fcallback"),
			authorization(allocated.port.wrapping_add(1), "dynamic_agent_client"),
			format!("{valid}&state=duplicate"),
			format!("{valid}#fragment"),
			valid.replace("state=server-state", "state="),
			valid.replace("client_id=dynamic_agent_client", "client_id="),
		] {
			assert!(relay.prepare(&allocated.id, &invalid).is_err(), "{invalid}");
		}
		assert!(relay.prepare(&allocated.id, &valid).is_ok());
	}

	#[tokio::test]
	async fn cancellation_releases_idle_and_active_listeners_and_rejects_late_delivery() {
		let relay = Relay::default();
		let idle = relay.listen().await.unwrap();
		let active = relay.listen().await.unwrap();
		let waiting = relay
			.prepare(
				&active.id,
				&authorization(active.port, "dynamic_agent_client"),
			)
			.unwrap();
		let receiving = tokio::spawn(waiting.receive());
		let mut late = tokio::net::TcpStream::connect(("127.0.0.1", active.port))
			.await
			.unwrap();
		late.write_all(b"GET /auth/callback?").await.unwrap();
		relay.cancel(&idle.id);
		assert!(
			tokio::net::TcpStream::connect(("127.0.0.1", idle.port))
				.await
				.is_err()
		);
		relay.cancel_all();
		let _ = late
			.write_all(b"code=late&state=server-state&client_id=oaiapp_issued HTTP/1.1\r\n\r\n")
			.await;
		assert!(receiving.await.unwrap().unwrap_err().contains("cancelled"));
		assert!(
			tokio::net::TcpStream::connect(("127.0.0.1", active.port))
				.await
				.is_err()
		);
		relay.cancel(&active.id);
	}

	#[tokio::test(start_paused = true)]
	async fn cancelled_flows_do_not_accumulate_expiration_tasks() {
		let relay = Relay::default();
		let runtime = tokio::runtime::Handle::current();
		let before = runtime.metrics().num_alive_tasks();
		for _ in 0..32 {
			let allocated = relay.listen().await.unwrap();
			relay.cancel(&allocated.id);
		}
		for _ in 0..4 {
			tokio::task::yield_now().await;
		}
		assert_eq!(runtime.metrics().num_alive_tasks(), before);
	}

	#[tokio::test(start_paused = true)]
	async fn timeout_releases_both_unclaimed_and_active_listeners() {
		let relay = Relay::default();
		let idle = relay.listen().await.unwrap();
		let active = relay.listen().await.unwrap();
		let waiting = relay
			.prepare(
				&active.id,
				&authorization(active.port, "dynamic_agent_client"),
			)
			.unwrap();
		let receiving = tokio::spawn(waiting.receive());
		tokio::task::yield_now().await;
		tokio::time::advance(std::time::Duration::from_secs(301)).await;
		assert!(receiving.await.unwrap().is_err());
		tokio::task::yield_now().await;
		assert!(
			relay
				.prepare(&idle.id, &authorization(idle.port, "dynamic_agent_client"))
				.is_err()
		);
		assert!(
			tokio::net::TcpStream::connect(("127.0.0.1", idle.port))
				.await
				.is_err()
		);
		assert!(
			tokio::net::TcpStream::connect(("127.0.0.1", active.port))
				.await
				.is_err()
		);
	}
}
