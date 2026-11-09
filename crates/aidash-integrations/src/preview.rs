//! The Preview gateway's public listener (ADR 0068). It serves only the
//! preview domain, exchanges a one-time ticket for a host-only cookie, adds the
//! confinement headers every Preview response carries, and proxies a Live
//! preview's HTTP and WebSocket traffic to its Pod (ADR 0069, ADR 0070).
use aidash_application::preview::{Authority, MAX_PAGE_BYTES, Proxy};
use aidash_domain::preview::{COOKIE, TICKET_PATH, content_security_policy};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, Empty, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{Request, Response, StatusCode, body::Frame, header, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
	convert::Infallible,
	pin::Pin,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
	time::Duration,
};
use tokio::{
	io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
	net::{TcpListener, TcpStream},
	sync::watch,
};

mod slots;
use slots::{Connection, Refusal, Slots};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
/// How long a client may take to send a request head, also while a kept-alive
/// connection waits for its next request. Browsers and an edge send at once.
const HEADER_READ: Duration = Duration::from_secs(10);
/// Refused connections answered at once, and for how long each. Anyone can
/// open connections, so answering them is bounded too; beyond this a connection
/// is dropped unanswered.
const REFUSAL_SLOTS: usize = 8;
const REFUSAL_WINDOW: Duration = Duration::from_secs(2);
const RETRY_AFTER_SECONDS: &str = "5";
/// A page reaches a Run's External services through its own origin at this
/// path; the Preview gateway forwards it to the Egress gateway (ADR 0069).
pub const EGRESS_PREFIX: &str = "/__aidash/egress/";
/// The largest request or buffered response the Preview gateway forwards.
const MAX_FORWARD_BYTES: usize = 64 * 1024 * 1024;

// Observe payload bytes in either direction, excluding the opening handshake.
struct ActiveStream<T> {
	inner: T,
	activity: Arc<AtomicBool>,
}

impl<T: AsyncRead + Unpin> AsyncRead for ActiveStream<T> {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		let before = buf.filled().len();
		let result = Pin::new(&mut self.inner).poll_read(cx, buf);
		if buf.filled().len() > before {
			self.activity.store(true, Ordering::Relaxed);
		}
		result
	}
}

impl<T: AsyncWrite + Unpin> AsyncWrite for ActiveStream<T> {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<std::io::Result<usize>> {
		Pin::new(&mut self.inner).poll_write(cx, buf)
	}
	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		Pin::new(&mut self.inner).poll_flush(cx)
	}
	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		Pin::new(&mut self.inner).poll_shutdown(cx)
	}
}

/// The cookie a preview subdomain receives. `__Host-` requires `Secure`,
/// `Path=/` and no `Domain`; framing the page cross-site requires `SameSite=None`.
pub fn session_cookie(value: &str, max_age: i64) -> String {
	format!("{COOKIE}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=None")
}

/// The session value from a request's `Cookie` header, if this gateway set one.
pub fn session_value(cookies: &str) -> Option<&str> {
	cookies
		.split(';')
		.map(str::trim)
		.find_map(|cookie| cookie.strip_prefix(COOKIE)?.strip_prefix('='))
		.filter(|value| !value.is_empty())
}

fn body(bytes: Bytes) -> Body {
	Full::new(bytes)
		.map_err(|never| match never {})
		.boxed_unsync()
}

fn empty() -> Body {
	Empty::<Bytes>::new()
		.map_err(|never| match never {})
		.boxed_unsync()
}

fn plain(status: StatusCode, message: &'static str) -> Response<Body> {
	Response::builder()
		.status(status)
		.header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
		.header(header::CACHE_CONTROL, "no-store")
		.header("referrer-policy", "no-referrer")
		.header("x-content-type-options", "nosniff")
		.body(body(Bytes::from_static(message.as_bytes())))
		.unwrap()
}

/// The answer to a connection or request turned away for want of a slot. The
/// connection closes after it, so the client starts over on a fresh one.
fn refused(refusal: Refusal) -> Response<Body> {
	let status = match refusal {
		Refusal::OtherPreview => StatusCode::MISDIRECTED_REQUEST,
		Refusal::Busy | Refusal::PreviewFull => StatusCode::SERVICE_UNAVAILABLE,
	};
	let mut response = plain(status, refusal.code());
	let headers = response.headers_mut();
	headers.insert(
		header::CONNECTION,
		header::HeaderValue::from_static("close"),
	);
	if status == StatusCode::SERVICE_UNAVAILABLE {
		headers.insert(
			header::RETRY_AFTER,
			header::HeaderValue::from_static(RETRY_AFTER_SECONDS),
		);
	}
	response
}

/// Moves the connection to the slot of the preview a request was admitted to.
fn admit(slots: &Slots, connection: &Connection, host: &str) -> Result<(), Refusal> {
	slots
		.admit(connection, host)
		.inspect_err(|refusal| slots.note(*refusal, "answered"))
}

/// Applies the confinement headers every Preview response carries.
fn error_response(error: aidash_application::Error) -> Response<Body> {
	use aidash_application::Error;
	match error {
		Error::Forbidden | Error::Unauthorized => plain(StatusCode::FORBIDDEN, "preview forbidden"),
		Error::NotFound(_) => plain(StatusCode::NOT_FOUND, "preview unavailable"),
		Error::Conflict(message) if message == "FILE_VERSION_TOO_LARGE" => {
			plain(StatusCode::PAYLOAD_TOO_LARGE, "file version too large")
		}
		Error::Conflict(message) if message == "PREVIEW_UNAVAILABLE" => {
			plain(StatusCode::SERVICE_UNAVAILABLE, "preview unavailable")
		}
		Error::Conflict(message) if message == "PREVIEW_EXPIRED" => {
			plain(StatusCode::GONE, "preview stopped")
		}
		_ => plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable"),
	}
}

async fn exchange(authority: &dyn Authority, host: &str, query: &str) -> Response<Body> {
	let ticket = query.split('&').find_map(|pair| {
		let value = pair.strip_prefix('t')?.strip_prefix('=')?;
		percent_encoding::percent_decode_str(value)
			.decode_utf8()
			.ok()
			.map(|value| value.into_owned())
	});
	let Some(ticket) = ticket else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	match authority.exchange(host, &ticket).await {
		Ok(exchange) => {
			let Ok(cookie) = header::HeaderValue::from_str(&session_cookie(
				exchange.cookie.expose(),
				exchange.max_age,
			)) else {
				return plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable");
			};
			Response::builder()
				.status(StatusCode::SEE_OTHER)
				.header(header::LOCATION, "/")
				.header(header::SET_COOKIE, cookie)
				.header(header::CACHE_CONTROL, "no-store")
				.header("referrer-policy", "no-referrer")
				.header("x-content-type-options", "nosniff")
				.body(body(Bytes::new()))
				.unwrap()
		}
		Err(error) => error_response(error),
	}
}

fn forwarded_request_header(name: &str) -> bool {
	matches!(
		name,
		"accept"
			| "accept-language"
			| "content-type"
			| "content-encoding"
			| "range" | "if-none-match"
			| "if-modified-since"
	)
}

fn forwarded_response_header(name: &str) -> bool {
	!matches!(
		name,
		"connection"
			| "keep-alive"
			| "transfer-encoding"
			| "te" | "trailer"
			| "upgrade"
			| "content-length"
			| "content-security-policy"
			| "content-security-policy-report-only"
	)
}

fn upstream_client() -> crate::Result<reqwest::Client> {
	reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.connect_timeout(Duration::from_secs(30))
		.build()
		.map_err(crate::http_error)
}

/// Proxy one HTTP request to the Live preview's Pod. The Pod is reachable only
/// from this gateway, and the response is streamed so an app's SSE still works.
async fn live_http(
	client: &reqwest::Client,
	proxy: &Proxy,
	request: Request<hyper::body::Incoming>,
) -> Response<Body> {
	let path = request
		.uri()
		.path_and_query()
		.map(|value| value.as_str())
		.unwrap_or("/")
		.to_owned();
	let method = match reqwest::Method::from_bytes(request.method().as_str().as_bytes()) {
		Ok(method) => method,
		Err(_) => return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden"),
	};
	let mut forwarded = client.request(method, format!("http://{}{path}", proxy.upstream));
	for (name, value) in request.headers() {
		if forwarded_request_header(name.as_str())
			&& let Ok(value) = value.to_str()
		{
			forwarded = forwarded.header(name.as_str(), value);
		}
	}
	let collected = match request.into_body().collect().await {
		Ok(collected) => collected.to_bytes(),
		Err(_) => return plain(StatusCode::BAD_REQUEST, "preview forbidden"),
	};
	if collected.len() > MAX_FORWARD_BYTES {
		return plain(StatusCode::PAYLOAD_TOO_LARGE, "preview request too large");
	}
	let upstream = match forwarded.body(collected).send().await {
		Ok(upstream) => upstream,
		Err(error) => {
			tracing::warn!(%error, "Live preview upstream unreachable");
			return plain(StatusCode::BAD_GATEWAY, "preview unavailable");
		}
	};
	let Some(policy) = content_security_policy(&proxy.frame_ancestor).ok() else {
		return plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable");
	};
	let status = upstream.status();
	let headers = upstream.headers().clone();
	let stream = upstream
		.bytes_stream()
		.map_ok(Frame::data)
		.map_err(|error| -> BoxError { std::io::Error::other(error.to_string()).into() });
	let mut builder = Response::builder().status(status);
	for (name, value) in headers.iter() {
		if forwarded_response_header(name.as_str()) {
			builder = builder.header(name.as_str(), value.as_bytes());
		}
	}
	builder
		.header("content-security-policy", policy)
		.header("x-content-type-options", "nosniff")
		.header("referrer-policy", "no-referrer")
		.header(header::CACHE_CONTROL, "no-store")
		.body(StreamBody::new(stream).boxed_unsync())
		.unwrap()
}

/// Forward `<EGRESS_PREFIX><service>/<path>` to the Egress gateway with the
/// Live preview's session token. No Tenant Secret is ever attached (ADR 0069).
async fn egress_forward(
	client: &reqwest::Client,
	proxy: &Proxy,
	request: Request<hyper::body::Incoming>,
) -> Response<Body> {
	let Some(origin) = proxy.egress_origin.as_deref() else {
		return plain(
			StatusCode::SERVICE_UNAVAILABLE,
			"external services unavailable",
		);
	};
	let Some(token) = proxy.egress_token.as_deref() else {
		return plain(
			StatusCode::SERVICE_UNAVAILABLE,
			"external services unavailable",
		);
	};
	let path = request.uri().path();
	let Some(target) = path.strip_prefix(EGRESS_PREFIX) else {
		return plain(StatusCode::NOT_FOUND, "preview unavailable");
	};
	let Some((service, rest)) = target.split_once('/') else {
		return plain(StatusCode::NOT_FOUND, "preview unavailable");
	};
	if service.is_empty() || rest.is_empty() || service.contains('/') {
		return plain(StatusCode::NOT_FOUND, "preview unavailable");
	}
	let query = request
		.uri()
		.query()
		.map(|query| format!("?{query}"))
		.unwrap_or_default();
	let url = format!(
		"{}/v1/{token}/{service}/{rest}{query}",
		origin.trim_end_matches('/')
	);
	let method = match reqwest::Method::from_bytes(request.method().as_str().as_bytes()) {
		Ok(method) => method,
		Err(_) => return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden"),
	};
	let mut forwarded = client.request(method, url);
	for (name, value) in request.headers() {
		if forwarded_request_header(name.as_str())
			&& let Ok(value) = value.to_str()
		{
			forwarded = forwarded.header(name.as_str(), value);
		}
	}
	let collected = match request.into_body().collect().await {
		Ok(collected) => collected.to_bytes(),
		Err(_) => return plain(StatusCode::BAD_REQUEST, "preview forbidden"),
	};
	if collected.len() > MAX_FORWARD_BYTES {
		return plain(StatusCode::PAYLOAD_TOO_LARGE, "preview request too large");
	}
	let upstream = match forwarded.body(collected).send().await {
		Ok(upstream) => upstream,
		Err(error) => {
			tracing::warn!(%error, "Live preview Egress forwarding failed");
			return plain(StatusCode::BAD_GATEWAY, "external service unavailable");
		}
	};
	let Some(policy) = content_security_policy(&proxy.frame_ancestor).ok() else {
		return plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable");
	};
	let status = upstream.status();
	let headers = upstream.headers().clone();
	let bytes = match upstream.bytes().await {
		Ok(bytes) => bytes,
		Err(_) => return plain(StatusCode::BAD_GATEWAY, "external service unavailable"),
	};
	if bytes.len() > MAX_FORWARD_BYTES {
		return plain(StatusCode::PAYLOAD_TOO_LARGE, "external response too large");
	}
	let mut builder = Response::builder().status(status);
	for (name, value) in headers.iter() {
		if forwarded_response_header(name.as_str()) {
			builder = builder.header(name.as_str(), value.as_bytes());
		}
	}
	builder
		.header("content-security-policy", policy)
		.header("x-content-type-options", "nosniff")
		.header("referrer-policy", "no-referrer")
		.header(header::CACHE_CONTROL, "no-store")
		.body(body(bytes))
		.unwrap()
}

fn websocket_request(request: &Request<hyper::body::Incoming>) -> bool {
	request
		.headers()
		.get(header::UPGRADE)
		.and_then(|value| value.to_str().ok())
		.is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Proxy a WebSocket handshake and tunnel both directions. Dev servers rely on
/// this for hot reload; the gateway is the only path into the Pod (ADR 0068).
async fn live_websocket(
	authority: &Arc<dyn Authority>,
	connection: &Connection,
	host: &str,
	cookie: &str,
	proxy: &Proxy,
	mut request: Request<hyper::body::Incoming>,
) -> Response<Body> {
	let path = request
		.uri()
		.path_and_query()
		.map(|value| value.as_str())
		.unwrap_or("/")
		.to_owned();
	let mut stream = match TcpStream::connect(&proxy.upstream).await {
		Ok(stream) => stream,
		Err(_) => return plain(StatusCode::BAD_GATEWAY, "preview unavailable"),
	};
	let mut head = format!(
		"GET {path} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n",
		proxy.upstream
	);
	for name in [
		"sec-websocket-key",
		"sec-websocket-version",
		"sec-websocket-protocol",
		"sec-websocket-extensions",
		"origin",
		"user-agent",
	] {
		if let Some(value) = request.headers().get(name)
			&& let Ok(value) = value.to_str()
		{
			head.push_str(&format!("{name}: {value}\r\n"));
		}
	}
	head.push_str("\r\n");
	if stream.write_all(head.as_bytes()).await.is_err() {
		return plain(StatusCode::BAD_GATEWAY, "preview unavailable");
	}
	let mut response = Vec::new();
	let mut buffer = [0_u8; 1024];
	loop {
		match stream.read(&mut buffer).await {
			Ok(0) => return plain(StatusCode::BAD_GATEWAY, "preview unavailable"),
			Ok(read) => response.extend_from_slice(&buffer[..read]),
			Err(_) => return plain(StatusCode::BAD_GATEWAY, "preview unavailable"),
		}
		if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
			let leftover = response.split_off(end + 4);
			let head = String::from_utf8_lossy(&response).into_owned();
			let status = head
				.lines()
				.next()
				.and_then(|line| line.split_whitespace().nth(1))
				.and_then(|code| code.parse::<u16>().ok())
				.unwrap_or(502);
			if status != 101 {
				return plain(StatusCode::BAD_GATEWAY, "preview unavailable");
			}
			let Some(policy) = content_security_policy(&proxy.frame_ancestor).ok() else {
				return plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable");
			};
			let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
			for line in head.lines().skip(1) {
				if let Some((name, value)) = line.split_once(':') {
					let name = name.trim().to_ascii_lowercase();
					if matches!(
						name.as_str(),
						"upgrade"
							| "connection" | "sec-websocket-accept"
							| "sec-websocket-protocol"
					) {
						builder = builder.header(name, value.trim());
					}
				}
			}
			let response = builder
				.header("content-security-policy", policy)
				.header("x-content-type-options", "nosniff")
				.header("referrer-policy", "no-referrer")
				.header(header::CACHE_CONTROL, "no-store")
				.body(empty())
				.unwrap();
			let upgrade = hyper::upgrade::on(&mut request);
			// Probe authority once a minute. Only bytes actually carried by the
			// tunnel refresh the idle deadline; the repository coalesces updates
			// across every socket of this preview.
			let authority = authority.clone();
			let host = host.to_owned();
			let cookie = cookie.to_owned();
			// The tunnel outlives the HTTP connection and keeps its slot.
			let slot = connection.tunnel();
			tokio::spawn(async move {
				let _slot = slot;
				let Ok(upgraded) = upgrade.await else {
					return;
				};
				let mut client = TokioIo::new(upgraded);
				if !leftover.is_empty() && client.write_all(&leftover).await.is_err() {
					return;
				}
				let activity = Arc::new(AtomicBool::new(false));
				let mut client = ActiveStream {
					inner: client,
					activity: activity.clone(),
				};
				let mut stream = ActiveStream {
					inner: stream,
					activity: activity.clone(),
				};
				let transfer = tokio::io::copy_bidirectional(&mut client, &mut stream);
				tokio::pin!(transfer);
				loop {
					tokio::select! {
						_ = &mut transfer => return,
						_ = tokio::time::sleep(Duration::from_secs(60)) => {
							let seen = activity.swap(false, Ordering::Relaxed);
							if !matches!(authority.live(&host, &cookie, seen).await, Ok(Some(_))) {
								return;
							}
						}
					}
				}
			});
			return response;
		}
		if response.len() > 64 * 1024 {
			return plain(StatusCode::BAD_GATEWAY, "preview unavailable");
		}
	}
}

async fn live(
	client: &reqwest::Client,
	authority: &Arc<dyn Authority>,
	connection: &Connection,
	host: &str,
	cookie: &str,
	proxy: Proxy,
	request: Request<hyper::body::Incoming>,
) -> Response<Body> {
	if websocket_request(&request) {
		return live_websocket(authority, connection, host, cookie, &proxy, request).await;
	}
	if request.uri().path().starts_with(EGRESS_PREFIX) {
		return egress_forward(client, &proxy, request).await;
	}
	live_http(client, &proxy, request).await
}

/// A File preview: only GET and HEAD, only the paths its manifest names.
/// `admit` runs once the viewer is confirmed and may still turn the page away.
async fn file_view(
	authority: &dyn Authority,
	host: &str,
	cookie: &str,
	method: &hyper::Method,
	path: &str,
	admit: impl FnOnce() -> Result<(), Refusal>,
) -> Response<Body> {
	let head = method == hyper::Method::HEAD;
	if method != hyper::Method::GET && !head {
		return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden");
	}
	match authority.view(host, cookie, path).await {
		Ok(page) => {
			if let Err(refusal) = admit() {
				return refused(refusal);
			}
			let Ok(policy) = content_security_policy(&page.frame_ancestor) else {
				return plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable");
			};
			if page.bytes.len() as u64 > MAX_PAGE_BYTES {
				return plain(StatusCode::PAYLOAD_TOO_LARGE, "file version too large");
			}
			let served = page.bytes.len() as u64;
			let mut builder = Response::builder()
				.status(StatusCode::OK)
				.header(header::CONTENT_TYPE, page.media_type.as_str())
				.header("content-security-policy", policy)
				.header("x-content-type-options", "nosniff")
				.header("referrer-policy", "no-referrer")
				.header(header::CACHE_CONTROL, "no-store")
				.header(header::CONTENT_LENGTH, served.to_string());
			if let Some(disposition) = page.content_disposition {
				builder = builder.header(header::CONTENT_DISPOSITION, disposition);
			}
			let response = builder
				.body(if head {
					body(Bytes::new())
				} else {
					body(Bytes::from(page.bytes))
				})
				.unwrap();
			if !head && let Err(error) = authority.served(&page.tenant, page.session, served).await
			{
				tracing::warn!(%error, "Preview viewing session could not be recorded");
			}
			response
		}
		Err(error) => error_response(error),
	}
}

async fn respond(
	client: reqwest::Client,
	authority: Arc<dyn Authority>,
	slots: &Slots,
	connection: &Connection,
	request: Request<hyper::body::Incoming>,
) -> Response<Body> {
	let host = request
		.headers()
		.get(header::HOST)
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned);
	let cookie = request
		.headers()
		.get(header::COOKIE)
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned);
	let method = request.method().clone();
	let path = request.uri().path().to_owned();
	let query = request.uri().query().unwrap_or_default().to_owned();
	let Some(host) = host else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	if path == TICKET_PATH {
		if method != hyper::Method::GET {
			return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden");
		}
		return exchange(authority.as_ref(), &host, &query).await;
	}
	let Some(cookie) = cookie.as_deref().and_then(session_value) else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	match authority.live(&host, cookie, true).await {
		Ok(Some(proxy)) => {
			if let Err(refusal) = admit(slots, connection, &host) {
				return refused(refusal);
			}
			live(
				&client, &authority, connection, &host, cookie, proxy, request,
			)
			.await
		}
		Ok(None) => {
			file_view(authority.as_ref(), &host, cookie, &method, &path, || {
				admit(slots, connection, &host)
			})
			.await
		}
		Err(error) => error_response(error),
	}
}

/// Answers a connection that found no free slot, within `REFUSAL_WINDOW`.
/// The answer goes out once the request head is read, so a request with a body
/// may see a reset instead; Preview requests rarely have one.
async fn refuse_connection(socket: TcpStream) {
	let service = service_fn(|_request| async { Ok::<_, Infallible>(refused(Refusal::Busy)) });
	let serving = hyper::server::conn::http1::Builder::new()
		.timer(TokioTimer::new())
		.keep_alive(false)
		.header_read_timeout(REFUSAL_WINDOW)
		.max_headers(64)
		.serve_connection(TokioIo::new(socket), service);
	let _ = tokio::time::timeout(REFUSAL_WINDOW, serving).await;
}

/// Serve until the supervisor stops. Requests are only ever the gateway's own
/// public traffic; the listener is bound on the Pod's declared address.
pub async fn serve(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	serve_with_slots(listener, authority, Arc::new(Slots::default()), stopping).await
}

async fn serve_with_slots(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	slots: Arc<Slots>,
	mut stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	let client = upstream_client()?;
	let refusals = Arc::new(tokio::sync::Semaphore::new(REFUSAL_SLOTS));
	let mut connections = tokio::task::JoinSet::new();
	let mut failures = crate::accept::AcceptFailures::new("preview gateway");
	loop {
		if *stopping.borrow() || stopping.has_changed().is_err() {
			connections.abort_all();
			return Ok(());
		}
		tokio::select! {
			_ = stopping.changed() => {},
			Some(_) = connections.join_next(), if !connections.is_empty() => {},
			accepted = listener.accept() => {
				let Some((socket, _)) = failures.settle(accepted, &mut stopping).await else { continue; };
				let Some(connection) = slots.arrive() else {
					let answering = refusals.clone().try_acquire_owned();
					slots.note(Refusal::Busy, if answering.is_ok() { "answered" } else { "dropped" });
					if let Ok(permit) = answering {
						connections.spawn(async move { let _permit = permit; refuse_connection(socket).await; });
					}
					continue;
				};
				let connection = Arc::new(connection);
				let authority = authority.clone();
				let client = client.clone();
				let slots = slots.clone();
				connections.spawn(async move {
					let service = service_fn(move |request| {
						let authority = authority.clone();
						let client = client.clone();
						let slots = slots.clone();
						let connection = connection.clone();
						async move {
							Ok::<_, Infallible>(respond(client, authority, &slots, &connection, request).await)
						}
					});
					let _ = hyper::server::conn::http1::Builder::new()
						.timer(TokioTimer::new())
						.header_read_timeout(HEADER_READ)
						.max_headers(64)
						.serve_connection(TokioIo::new(socket), service)
						.with_upgrades()
						.await;
				});
			}
		}
	}
}

/// The file and ticket routing without proxying, for the gateway's own tests.
#[cfg(test)]
async fn dispatch(
	authority: &dyn Authority,
	host: Option<&str>,
	method: &hyper::Method,
	path: &str,
	query: &str,
	cookie: Option<&str>,
) -> Response<Body> {
	let Some(host) = host else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	if path == TICKET_PATH {
		if method != hyper::Method::GET {
			return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden");
		}
		return exchange(authority, host, query).await;
	}
	let Some(cookie) = cookie.and_then(session_value) else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	match authority.live(host, cookie, true).await {
		Ok(Some(_)) => plain(StatusCode::INTERNAL_SERVER_ERROR, "live proxy under test"),
		Ok(None) => file_view(authority, host, cookie, method, path, || Ok(())).await,
		Err(error) => error_response(error),
	}
}

#[cfg(test)]
mod tests;
