//! Scope-authenticated forward proxy.

use aidash_application::egress::{Direction, EgressScopes, ScopeSettings};
use http_body_util::BodyExt;
use std::{
	io,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const BATCH: u64 = 1024 * 1024;

/// Reserve before forwarding. A crash can leave unused reservation charged,
/// but never permits traffic beyond the durable daily budget.
struct Batch {
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
	direction: Direction,
	day: chrono::NaiveDate,
	remaining: u64,
}
impl Batch {
	fn new(scopes: Arc<dyn EgressScopes>, settings: ScopeSettings, direction: Direction) -> Self {
		Self {
			scopes,
			settings,
			direction,
			day: chrono::Utc::now().date_naive(),
			remaining: 0,
		}
	}
	async fn charge(&mut self, bytes: usize) -> io::Result<()> {
		self.scopes
			.check_active(&self.settings)
			.await
			.map_err(|e| io::Error::other(e.to_string()))?;
		let day = chrono::Utc::now().date_naive();
		if day != self.day {
			if self.remaining != 0 {
				self.scopes
					.refund(&self.settings, self.direction, self.remaining, self.day)
					.await
					.map_err(|e| io::Error::other(e.to_string()))?;
			}
			self.day = day;
			self.remaining = 0;
		}
		let bytes = bytes as u64;
		if self.remaining < bytes {
			let need = bytes - self.remaining;
			let amount = BATCH.max(need);
			match self
				.scopes
				.debit(&self.settings, self.direction, amount, day)
				.await
			{
				Ok(()) => self.remaining += amount,
				Err(aidash_application::Error::Refused(code))
					if code == "egress_budget_exhausted" && amount != need =>
				{
					self.scopes
						.debit(&self.settings, self.direction, need, day)
						.await
						.map_err(|e| io::Error::other(e.to_string()))?;
					self.remaining += need;
				}
				Err(error) => return Err(io::Error::other(error.to_string())),
			}
		}
		self.remaining -= bytes;
		Ok(())
	}
}
impl Drop for Batch {
	fn drop(&mut self) {
		if self.remaining == 0 {
			return;
		}
		let scopes = self.scopes.clone();
		let settings = self.settings.clone();
		let direction = self.direction;
		let day = self.day;
		let remaining = self.remaining;
		tokio::spawn(async move {
			if scopes
				.refund(&settings, direction, remaining, day)
				.await
				.is_err()
			{
				tracing::error!("Egress reservation refund failed");
			}
		});
	}
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	mut read: R,
	mut write: W,
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
	direction: Direction,
	count: Arc<AtomicU64>,
) -> io::Result<()> {
	let mut buffer = vec![0u8; BATCH as usize];
	let mut batch = Batch::new(scopes, settings, direction);
	loop {
		let n = read.read(&mut buffer).await?;
		if n == 0 {
			write.shutdown().await?;
			return Ok(());
		}
		batch.charge(n).await?;
		let mut written = 0;
		while written < n {
			match write.write(&buffer[written..n]).await {
				Ok(0) => {
					batch.remaining += (n - written) as u64;
					return Err(io::Error::new(io::ErrorKind::WriteZero, "upstream closed"));
				}
				Ok(bytes) => {
					written += bytes;
					count.fetch_add(bytes as u64, Ordering::Relaxed);
				}
				Err(error) => {
					batch.remaining += (n - written) as u64;
					return Err(error);
				}
			}
		}
	}
}

async fn tunnel<C, U>(
	client: C,
	upstream: U,
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
) -> (u64, u64, io::Result<()>)
where
	C: AsyncRead + AsyncWrite + Unpin,
	U: AsyncRead + AsyncWrite + Unpin,
{
	let (client_read, client_write) = tokio::io::split(client);
	let (upstream_read, upstream_write) = tokio::io::split(upstream);
	let sent = Arc::new(AtomicU64::new(0));
	let received = Arc::new(AtomicU64::new(0));
	let result = tokio::try_join!(
		pump(
			client_read,
			upstream_write,
			scopes.clone(),
			settings.clone(),
			Direction::Sent,
			sent.clone()
		),
		pump(
			upstream_read,
			client_write,
			scopes,
			settings,
			Direction::Received,
			received.clone()
		),
	)
	.map(|_| ());
	(
		sent.load(Ordering::Relaxed),
		received.load(Ordering::Relaxed),
		result,
	)
}

/// Resolve once and connect to the verified addresses, preventing DNS rebinding.
async fn public_addresses(host: &str, port: u16) -> io::Result<Vec<std::net::SocketAddr>> {
	let addresses: Vec<_> = tokio::net::lookup_host((host, port)).await?.collect();
	if addresses.is_empty()
		|| addresses
			.iter()
			.any(|address| !aidash_domain::capabilities::outbound::public_ip(address.ip()))
	{
		return Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"destination_forbidden",
		));
	}
	Ok(addresses)
}

struct Audit {
	scopes: Arc<dyn EgressScopes>,
	scope: aidash_domain::egress::ScopeId,
	host: String,
	port: u16,
	started: std::time::Instant,
	sent: Arc<AtomicU64>,
	received: Arc<AtomicU64>,
	outcome: &'static str,
}
impl Drop for Audit {
	fn drop(&mut self) {
		let scopes = self.scopes.clone();
		let record = aidash_application::egress::ConnectionRecord {
			scope: self.scope.clone(),
			host: self.host.clone(),
			port: self.port,
			sent_bytes: self.sent.load(Ordering::Relaxed),
			received_bytes: self.received.load(Ordering::Relaxed),
			duration_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
			outcome: self.outcome.to_owned(),
		};
		tokio::spawn(async move {
			if scopes.record(record).await.is_err() {
				tracing::error!("Egress connection audit failed");
			}
		});
	}
}

fn credential(headers: &hyper::HeaderMap) -> Option<String> {
	use base64::Engine;
	let mut values = headers.get_all(hyper::header::PROXY_AUTHORIZATION).iter();
	let value = values.next()?.to_str().ok()?;
	if values.next().is_some() || value.len() > 4096 {
		return None;
	}
	let (scheme, value) = value.split_once(' ')?;
	if scheme.eq_ignore_ascii_case("bearer") && !value.is_empty() {
		return Some(value.to_owned());
	}
	if scheme.eq_ignore_ascii_case("basic") {
		let decoded = base64::engine::general_purpose::STANDARD
			.decode(value)
			.ok()?;
		let decoded = std::str::from_utf8(&decoded).ok()?;
		let (_, token) = decoded.split_once(':')?;
		if !token.is_empty() {
			return Some(token.to_owned());
		}
	}
	None
}

fn auth_required() -> hyper::Response<super::Body> {
	let mut response = super::failure(
		hyper::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
		"proxy_auth_required",
	);
	response.headers_mut().insert(
		hyper::header::PROXY_AUTHENTICATE,
		hyper::header::HeaderValue::from_static("Basic realm=\"aidash-egress\""),
	);
	response
}

fn refusal(error: &aidash_application::Error) -> (hyper::StatusCode, &'static str) {
	use aidash_application::Error;
	match error {
		Error::Unauthorized => (
			hyper::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
			"proxy_auth_required",
		),
		Error::Forbidden => (hyper::StatusCode::FORBIDDEN, "egress_forbidden"),
		Error::Refused(code) if code == "egress_budget_exhausted" => (
			hyper::StatusCode::TOO_MANY_REQUESTS,
			"egress_budget_exhausted",
		),
		Error::Refused(code) if code == "egress_suspended" => {
			(hyper::StatusCode::FORBIDDEN, "egress_suspended")
		}
		_ => (hyper::StatusCode::SERVICE_UNAVAILABLE, "egress_unavailable"),
	}
}

pub(super) async fn forward(
	request: hyper::Request<hyper::body::Incoming>,
	scopes: Arc<dyn EgressScopes>,
	permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> hyper::Response<super::Body> {
	use hyper::{Method, StatusCode};
	let Some(token) = credential(request.headers()) else {
		return auth_required();
	};
	let scope = match scopes.resolve(&token).await {
		Ok(scope) => scope,
		Err(error) => {
			let (status, code) = refusal(&error);
			if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
				return auth_required();
			}
			return super::failure(status, code);
		}
	};
	let connect = request.method() == Method::CONNECT;
	let parsed = if connect {
		request
			.uri()
			.authority()
			.and_then(|a| url::Url::parse(&format!("https://{a}/")).ok())
	} else {
		url::Url::parse(&request.uri().to_string()).ok()
	};
	let Some(url) = parsed.filter(|u| {
		u.username().is_empty()
			&& u.password().is_none()
			&& u.fragment().is_none()
			&& ((connect && u.scheme() == "https" && u.port_or_known_default() == Some(443))
				|| (!connect && u.scheme() == "http" && u.port_or_known_default() == Some(80)))
	}) else {
		return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	let Some(host) = url.host_str() else {
		return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	let host = host.to_owned();
	let port = url.port_or_known_default().unwrap();
	let mut audit = Audit {
		scopes: scopes.clone(),
		scope: scope.id.clone(),
		host: host.clone(),
		port,
		started: std::time::Instant::now(),
		sent: Arc::new(AtomicU64::new(0)),
		received: Arc::new(AtomicU64::new(0)),
		outcome: "refused",
	};
	if !scope
		.allowed_hosts
		.permits(&host, scope.operator_allows_all)
	{
		return super::failure(StatusCode::FORBIDDEN, "egress_host_forbidden");
	}
	if let Err(error) = scopes.admit_connection(&scope).await {
		let (status, code) = refusal(&error);
		return super::failure(status, code);
	}
	let addresses = match public_addresses(&host, port).await {
		Ok(addresses) => addresses,
		Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
			return super::failure(StatusCode::FORBIDDEN, "egress_destination_forbidden");
		}
		Err(_) => {
			audit.outcome = "dns_error";
			return super::failure(StatusCode::BAD_GATEWAY, "egress_dns_failed");
		}
	};
	if connect {
		let upstream = match tokio::time::timeout(
			super::IDLE,
			tokio::net::TcpStream::connect(addresses.as_slice()),
		)
		.await
		{
			Ok(Ok(upstream)) => upstream,
			_ => {
				audit.outcome = "connect_error";
				return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed");
			}
		};
		return connect_tunnel(request, upstream, scopes, scope, audit, permit);
	}
	forward_http(request, url, addresses, scopes, scope, audit).await
}

fn connect_tunnel(
	mut request: hyper::Request<hyper::body::Incoming>,
	upstream: tokio::net::TcpStream,
	scopes: Arc<dyn EgressScopes>,
	scope: ScopeSettings,
	mut audit: Audit,
	permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> hyper::Response<super::Body> {
	let sent = audit.sent.clone();
	let received = audit.received.clone();
	let upgrade = hyper::upgrade::on(&mut request);
	tokio::spawn(async move {
		let _permit = permit;
		match upgrade.await {
			Ok(client) => {
				let (sent_bytes, received_bytes, result) = tunnel(
					hyper_util::rt::TokioIo::new(client),
					upstream,
					scopes,
					scope,
				)
				.await;
				sent.store(sent_bytes, Ordering::Relaxed);
				received.store(received_bytes, Ordering::Relaxed);
				audit.outcome = match result {
					Ok(()) => "completed",
					Err(error) if error.to_string().contains("egress_budget_exhausted") => {
						"egress_budget_exhausted"
					}
					Err(error) if error.to_string().contains("egress_suspended") => {
						"egress_suspended"
					}
					Err(_) => "interrupted",
				};
			}
			Err(_) => audit.outcome = "upgrade_failed",
		}
		drop(audit);
	});
	hyper::Response::builder()
		.status(hyper::StatusCode::OK)
		.body(
			http_body_util::Full::new(bytes::Bytes::new())
				.map_err(|never| match never {})
				.boxed_unsync(),
		)
		.unwrap()
}

async fn forward_http(
	request: hyper::Request<hyper::body::Incoming>,
	url: url::Url,
	addresses: Vec<std::net::SocketAddr>,
	scopes: Arc<dyn EgressScopes>,
	scope: ScopeSettings,
	mut audit: Audit,
) -> hyper::Response<super::Body> {
	use hyper::{StatusCode, header};
	let upstream = match tokio::time::timeout(
		super::IDLE,
		tokio::net::TcpStream::connect(addresses.as_slice()),
	)
	.await
	{
		Ok(Ok(socket)) => socket,
		_ => {
			audit.outcome = "connect_error";
			return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed");
		}
	};
	let (client, proxy) = tokio::io::duplex(64 * 1024);
	let sent = audit.sent.clone();
	let received = audit.received.clone();
	let mut transfer = tokio::spawn(async move {
		let (sent_bytes, received_bytes, result) = tunnel(proxy, upstream, scopes, scope).await;
		sent.store(sent_bytes, Ordering::Relaxed);
		received.store(received_bytes, Ordering::Relaxed);
		let outcome = match result {
			Ok(()) => "completed",
			Err(error) if error.to_string().contains("egress_budget_exhausted") => {
				"egress_budget_exhausted"
			}
			Err(error) if error.to_string().contains("egress_suspended") => "egress_suspended",
			Err(_) => "interrupted",
		};
		audit.outcome = outcome;
		drop(audit);
		outcome
	});
	let (mut sender, connection) =
		match hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(client)).await {
			Ok(pair) => pair,
			Err(_) => return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed"),
		};
	tokio::spawn(async move {
		let _ = connection.await;
	});
	let (mut parts, body) = request.into_parts();
	let target = match url[url::Position::BeforePath..].parse() {
		Ok(target) => target,
		Err(_) => return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target"),
	};
	parts.uri = target;
	let connection_headers = parts
		.headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in parts.headers.keys().cloned().collect::<Vec<_>>() {
		if matches!(
			name.as_str(),
			"proxy-authorization"
				| "proxy-authenticate"
				| "connection"
				| "keep-alive"
				| "transfer-encoding"
				| "upgrade" | "te"
				| "trailer" | "host"
		) || connection_headers.iter().any(|item| item == name.as_str())
		{
			parts.headers.remove(name);
		}
	}
	let host = &url[url::Position::BeforeHost..url::Position::AfterPort];
	let Ok(host) = header::HeaderValue::from_str(host) else {
		return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	parts.headers.insert(header::HOST, host);
	parts.headers.insert(
		header::CONNECTION,
		header::HeaderValue::from_static("close"),
	);
	let response = tokio::select! {
		response = sender.send_request(hyper::Request::from_parts(parts, body)) => match response {
			Ok(response) => response,
			Err(_) => return super::failure(StatusCode::BAD_GATEWAY, "egress_upstream_failed"),
		},
		outcome = &mut transfer => {
			return match outcome {
				Ok("egress_budget_exhausted") => super::failure(StatusCode::TOO_MANY_REQUESTS, "egress_budget_exhausted"),
				Ok("egress_suspended") => super::failure(StatusCode::FORBIDDEN, "egress_suspended"),
				_ => super::failure(StatusCode::BAD_GATEWAY, "egress_upstream_failed"),
			};
		}
	};
	let (mut parts, body) = response.into_parts();
	let connection_headers = parts
		.headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in parts.headers.keys().cloned().collect::<Vec<_>>() {
		if matches!(
			name.as_str(),
			"proxy-authenticate"
				| "connection"
				| "keep-alive"
				| "transfer-encoding"
				| "upgrade" | "te"
				| "trailer"
		) || connection_headers.iter().any(|item| item == name.as_str())
		{
			parts.headers.remove(name);
		}
	}
	hyper::Response::from_parts(
		parts,
		body.map_err(|error| Box::new(error) as super::BoxError)
			.boxed_unsync(),
	)
}

#[cfg(test)]
mod tests {
	use aidash_application::egress::{EgressScopes, ScopeSettings, test_support::MemoryScopes};
	use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget, ScopeId};
	use std::sync::Arc;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	#[tokio::test]
	async fn tunnel_counts_both_directions_and_enforces_budget() {
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget {
				sent_bytes: 3,
				received_bytes: 3,
			},
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let (mut client, gateway_client) = tokio::io::duplex(32);
		let (gateway_server, mut server) = tokio::io::duplex(32);
		let task = tokio::spawn(super::tunnel(
			gateway_client,
			gateway_server,
			scopes,
			settings,
		));
		client.write_all(b"abc").await.unwrap();
		let mut data = [0; 3];
		server.read_exact(&mut data).await.unwrap();
		assert_eq!(&data, b"abc");
		server.write_all(b"xyz").await.unwrap();
		client.read_exact(&mut data).await.unwrap();
		assert_eq!(&data, b"xyz");
		client.write_all(b"q").await.unwrap();
		let result = task.await.unwrap();
		assert_eq!(result.0, 3);
		assert_eq!(result.1, 3);
		assert!(result.2.is_err());
	}
	struct DenyLegacy;
	#[async_trait::async_trait]
	impl aidash_application::egress::Authority for DenyLegacy {
		async fn admit(
			&self,
			_: &str,
			_: &str,
			_: &str,
			_: &str,
			_: &str,
		) -> aidash_application::Result<aidash_application::egress::Admission> {
			Err(aidash_application::Error::Forbidden)
		}
	}

	#[tokio::test]
	async fn proxy_requires_scope_credential_and_rejects_private_resolution() {
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts {
				hosts: vec!["127.0.0.1".into()],
				allow_all: false,
			},
			budget: DailyBudget::default(),
			anomaly: AnomalyRule {
				connections_per_minute: 2,
			},
			operator_allows_all: false,
		};
		let scope_id = settings.id.clone();
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, stopping) = tokio::sync::watch::channel(false);
		let gateway = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			stopping,
		));
		async fn request(address: std::net::SocketAddr, auth: &str) -> String {
			let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
			socket
				.write_all(
					format!(
						"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n{auth}Connection: close\r\n\r\n"
					)
					.as_bytes(),
				)
				.await
				.unwrap();
			let mut response = String::new();
			socket.read_to_string(&mut response).await.unwrap();
			response
		}
		assert!(request(address, "").await.starts_with("HTTP/1.1 407"));
		assert!(
			request(address, "Proxy-Authorization: Bearer invalid\r\n")
				.await
				.starts_with("HTTP/1.1 407")
		);
		assert!(
			request(address, "Proxy-Authorization: Bearer token\r\n")
				.await
				.starts_with("HTTP/1.1 403")
		);
		let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
		socket.write_all(b"GET http://127.0.0.1/private?secret=value HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = String::new();
		socket.read_to_string(&mut response).await.unwrap();
		assert!(response.starts_with("HTTP/1.1 403"));
		let suspended = request(address, "Proxy-Authorization: Bearer token\r\n").await;
		assert!(suspended.contains("egress_suspended"));
		assert_eq!(scopes.notifications(), 1);
		scopes.lift_suspension(&scope_id).await.unwrap();
		let lifted = request(address, "Proxy-Authorization: Bearer token\r\n").await;
		assert!(lifted.contains("egress_destination_forbidden"));
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes.records().is_empty() {
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].host, "127.0.0.1");
		assert_eq!(records[0].outcome, "refused");
		stop.send(true).unwrap();
		gateway.await.unwrap().unwrap();
	}
	#[tokio::test]
	async fn absolute_http_forwards_without_proxy_credentials_and_records_wire_bytes() {
		use hyper::{Request, Response, service::service_fn};
		use std::convert::Infallible;
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (socket, _) = upstream.accept().await.unwrap();
			let service = service_fn(|request: Request<hyper::body::Incoming>| async move {
				assert!(request.headers().get("proxy-authorization").is_none());
				assert_eq!(
					request.uri().path_and_query().unwrap().as_str(),
					"/private?secret=value"
				);
				Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
					bytes::Bytes::from_static(b"hello"),
				)))
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await
				.unwrap();
		});
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget::default(),
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let gateway_address = gateway.local_addr().unwrap();
		let gateway_scopes = scopes.clone();
		let gateway_task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let scopes = gateway_scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "test.invalid".into(),
						port: 80,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
					};
					let result = super::forward_http(
						request,
						url::Url::parse("http://test.invalid/private?secret=value").unwrap(),
						vec![upstream_address],
						scopes,
						settings,
						audit,
					)
					.await;
					Ok::<_, Infallible>(result)
				}
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await
				.unwrap();
		});
		let mut client = tokio::net::TcpStream::connect(gateway_address)
			.await
			.unwrap();
		client.write_all(b"GET http://test.invalid/private?secret=value HTTP/1.1\r\nHost: test.invalid\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = Vec::new();
		client.read_to_end(&mut response).await.unwrap();
		assert!(response.ends_with(b"hello"));
		upstream_task.await.unwrap();
		gateway_task.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes.records().is_empty() {
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].host, "test.invalid");
		assert!(records[0].sent_bytes > 5);
		assert!(records[0].received_bytes > 5);
	}
	#[tokio::test]
	async fn connect_upgrades_and_counts_tunnel_bytes() {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (mut socket, _) = upstream.accept().await.unwrap();
			let mut input = [0; 5];
			socket.read_exact(&mut input).await.unwrap();
			assert_eq!(&input, b"hello");
			socket.write_all(b"world").await.unwrap();
			socket.shutdown().await.unwrap();
		});
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget::default(),
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let gateway_scopes = scopes.clone();
		let gateway_task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let scopes = gateway_scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "test.invalid".into(),
						port: 443,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
					};
					let upstream = tokio::net::TcpStream::connect(upstream_address)
						.await
						.unwrap();
					let permit = Arc::new(
						Arc::new(tokio::sync::Semaphore::new(1))
							.acquire_owned()
							.await
							.unwrap(),
					);
					Ok::<_, Infallible>(super::connect_tunnel(
						request, upstream, scopes, settings, audit, permit,
					))
				}
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.with_upgrades()
				.await
				.unwrap();
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client
			.write_all(b"CONNECT test.invalid:443 HTTP/1.1\r\nHost: test.invalid:443\r\n\r\n")
			.await
			.unwrap();
		let mut head = Vec::new();
		while !head.ends_with(b"\r\n\r\n") {
			head.push(client.read_u8().await.unwrap());
		}
		assert!(head.starts_with(b"HTTP/1.1 200"));
		client.write_all(b"hello").await.unwrap();
		client.shutdown().await.unwrap();
		let mut reply = [0; 5];
		client.read_exact(&mut reply).await.unwrap();
		assert_eq!(&reply, b"world");
		upstream_task.await.unwrap();
		gateway_task.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes.records().is_empty() {
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].sent_bytes, 5);
		assert_eq!(records[0].received_bytes, 5);
		assert_eq!(records[0].outcome, "completed");
	}
}
