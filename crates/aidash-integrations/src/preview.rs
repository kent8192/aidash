//! The Preview gateway's public listener (ADR 0068). It serves only the
//! preview domain, exchanges a one-time ticket for a host-only cookie, and adds
//! the confinement headers every Preview response carries.
use aidash_application::preview::{Authority, MAX_PAGE_BYTES};
use aidash_domain::preview::{COOKIE, TICKET_PATH, content_security_policy};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{Request, Response, StatusCode, body::Incoming, header, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
const HEADER_READ: Duration = Duration::from_secs(30);

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

async fn exchange(authority: &dyn Authority, host: &str, query: &str) -> Response<Body> {
	let ticket = query.split('&').find_map(|pair| {
		let value = pair.strip_prefix("t=")?;
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
		Err(aidash_application::Error::Forbidden | aidash_application::Error::Unauthorized) => {
			plain(StatusCode::FORBIDDEN, "preview forbidden")
		}
		Err(aidash_application::Error::NotFound(_)) => {
			plain(StatusCode::NOT_FOUND, "preview unavailable")
		}
		Err(aidash_application::Error::Conflict(message))
			if message == "FILE_VERSION_TOO_LARGE" =>
		{
			plain(StatusCode::PAYLOAD_TOO_LARGE, "file version too large")
		}
		Err(aidash_application::Error::Conflict(message)) if message == "PREVIEW_UNAVAILABLE" => {
			plain(StatusCode::SERVICE_UNAVAILABLE, "preview unavailable")
		}
		Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable"),
	}
}

async fn respond(authority: Arc<dyn Authority>, request: Request<Incoming>) -> Response<Body> {
	let host = request
		.headers()
		.get(header::HOST)
		.and_then(|value| value.to_str().ok());
	let cookie = request
		.headers()
		.get(header::COOKIE)
		.and_then(|value| value.to_str().ok());
	dispatch(
		authority.as_ref(),
		host,
		request.method(),
		request.uri().path(),
		request.uri().query().unwrap_or_default(),
		cookie,
	)
	.await
}

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
	let head = method == hyper::Method::HEAD;
	if method != hyper::Method::GET && !head {
		return plain(StatusCode::METHOD_NOT_ALLOWED, "preview forbidden");
	}
	let Some(cookie) = cookie.and_then(session_value) else {
		return plain(StatusCode::FORBIDDEN, "preview forbidden");
	};
	match authority.view(host, cookie, path).await {
		Ok(page) => {
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
		Err(aidash_application::Error::Forbidden | aidash_application::Error::Unauthorized) => {
			plain(StatusCode::FORBIDDEN, "preview forbidden")
		}
		Err(aidash_application::Error::NotFound(_)) => {
			plain(StatusCode::NOT_FOUND, "preview unavailable")
		}
		Err(aidash_application::Error::Conflict(message))
			if message == "FILE_VERSION_TOO_LARGE" =>
		{
			plain(StatusCode::PAYLOAD_TOO_LARGE, "file version too large")
		}
		Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "preview unavailable"),
	}
}

/// Serve until the supervisor stops. Requests are only ever the gateway's own
/// public traffic; the listener is bound on the Pod's declared address.
pub async fn serve(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	mut stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	let mut connections = tokio::task::JoinSet::new();
	loop {
		if *stopping.borrow() {
			connections.abort_all();
			return Ok(());
		}
		tokio::select! {
			_ = stopping.changed() => {},
			Some(_) = connections.join_next(), if !connections.is_empty() => {},
			accepted = listener.accept() => {
				let (socket, _) = accepted.map_err(|_| aidash_application::Error::External(
					"Preview listener unavailable".into(),
				))?;
				let authority = authority.clone();
				connections.spawn(async move {
					let service = service_fn(move |request| {
						let authority = authority.clone();
						async move { Ok::<_, Infallible>(respond(authority, request).await) }
					});
					let _ = hyper::server::conn::http1::Builder::new()
						.header_read_timeout(HEADER_READ)
						.max_headers(64)
						.serve_connection(TokioIo::new(socket), service)
						.await;
				});
			}
		}
	}
}

#[cfg(test)]
mod tests;
