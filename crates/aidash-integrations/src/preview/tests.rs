use super::*;

#[test]
fn the_session_cookie_is_host_only_and_framed_cross_site() {
	let cookie = session_cookie("secret", 3600);
	assert_eq!(
		cookie,
		"__Host-aidash-preview=secret; Path=/; Max-Age=3600; Secure; HttpOnly; SameSite=None"
	);
	assert!(cookie.contains("__Host-"));
	assert!(!cookie.contains("Domain"));
	assert!(cookie.contains("Path=/"));
	assert!(cookie.contains("Secure"));
	assert!(cookie.contains("HttpOnly"));
	assert!(cookie.contains("SameSite=None"));
}

#[test]
fn only_this_gateways_cookie_is_read() {
	assert_eq!(
		session_value("other=1; __Host-aidash-preview=abc; more=2"),
		Some("abc")
	);
	assert_eq!(session_value("__Host-aidash-preview="), None);
	assert_eq!(session_value("__Host-aidash-preview"), None);
	assert_eq!(session_value("other=1"), None);
}

use aidash_application::preview::{Authority, Exchange, Page};
use async_trait::async_trait;
use std::sync::Mutex;

struct Fake {
	page: Option<Page>,
	revoked: bool,
	served: Mutex<Vec<(String, u64)>>,
}

#[async_trait]
impl Authority for Fake {
	async fn exchange(&self, _: &str, _: &str) -> aidash_application::Result<Exchange> {
		Ok(Exchange {
			cookie: aidash_domain::preview::PreviewSecret::parse("s".repeat(43)).unwrap(),
			max_age: 3600,
		})
	}
	async fn view(&self, _: &str, _: &str, _: &str) -> aidash_application::Result<Page> {
		if self.revoked {
			return Err(aidash_application::Error::Forbidden);
		}
		self.page
			.clone()
			.ok_or_else(|| aidash_application::Error::NotFound("file".into()))
	}
	async fn live(
		&self,
		_: &str,
		_: &str,
	) -> aidash_application::Result<Option<aidash_application::preview::Proxy>> {
		Ok(None)
	}
	async fn served(
		&self,
		tenant: &str,
		_: uuid::Uuid,
		bytes: u64,
	) -> aidash_application::Result<()> {
		self.served.lock().unwrap().push((tenant.into(), bytes));
		Ok(())
	}
}

fn page(media_type: &str, disposition: Option<&str>) -> Page {
	Page {
		tenant: "acme".into(),
		session: uuid::Uuid::new_v4(),
		frame_ancestor: "https://dashboard.example".into(),
		media_type: media_type.into(),
		content_disposition: disposition.map(str::to_owned),
		bytes: b"<!doctype html>".to_vec(),
	}
}

async fn body(
	response: hyper::Response<Body>,
) -> (hyper::http::StatusCode, hyper::HeaderMap, Vec<u8>) {
	let status = response.status();
	let headers = response.headers().clone();
	let bytes = response
		.into_body()
		.collect()
		.await
		.unwrap()
		.to_bytes()
		.to_vec();
	(status, headers, bytes)
}

#[tokio::test]
async fn a_ticket_exchange_sets_the_host_only_cookie_and_redirects() {
	let fake = Fake {
		page: None,
		revoked: false,
		served: Mutex::new(Vec::new()),
	};
	let response = dispatch(
		&fake,
		Some("label.aidashusercontent.app"),
		&hyper::Method::GET,
		TICKET_PATH,
		"t=ticket",
		None,
	)
	.await;
	let (status, headers, _) = body(response).await;
	assert_eq!(status, hyper::StatusCode::SEE_OTHER);
	assert_eq!(headers[hyper::header::LOCATION], "/");
	let cookie = headers[hyper::header::SET_COOKIE].to_str().unwrap();
	assert!(cookie.starts_with("__Host-aidash-preview="));
	assert!(cookie.contains("SameSite=None"));
	assert!(cookie.contains("Secure"));
	assert!(cookie.contains("HttpOnly"));
	assert!(!cookie.contains("Domain"));
}

#[tokio::test]
async fn a_preview_response_carries_the_confinement_headers() {
	let fake = Fake {
		page: Some(page("text/html; charset=utf-8", None)),
		revoked: false,
		served: Mutex::new(Vec::new()),
	};
	let response = dispatch(
		&fake,
		Some("label.aidashusercontent.app"),
		&hyper::Method::GET,
		"/",
		"",
		Some("__Host-aidash-preview=session"),
	)
	.await;
	let (status, headers, bytes) = body(response).await;
	assert_eq!(status, hyper::StatusCode::OK);
	assert_eq!(
		headers[hyper::header::CONTENT_TYPE],
		"text/html; charset=utf-8"
	);
	assert_eq!(
		headers["content-security-policy"],
		"default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; media-src 'self'; worker-src 'self' blob:; frame-ancestors https://dashboard.example; base-uri 'self'; form-action 'self'; object-src 'none'"
	);
	assert_eq!(headers["x-content-type-options"], "nosniff");
	assert_eq!(headers["referrer-policy"], "no-referrer");
	assert_eq!(headers[hyper::header::CACHE_CONTROL], "no-store");
	assert!(!headers.contains_key(hyper::header::CONTENT_DISPOSITION));
	assert_eq!(bytes, b"<!doctype html>");
	assert_eq!(*fake.served.lock().unwrap(), [("acme".into(), 15)]);
}

#[tokio::test]
async fn a_pdf_response_is_inline() {
	let fake = Fake {
		page: Some(page("application/pdf", Some("inline"))),
		revoked: false,
		served: Mutex::new(Vec::new()),
	};
	let response = dispatch(
		&fake,
		Some("label.aidashusercontent.app"),
		&hyper::Method::GET,
		"/",
		"",
		Some("__Host-aidash-preview=session"),
	)
	.await;
	let (_, headers, _) = body(response).await;
	assert_eq!(headers[hyper::header::CONTENT_TYPE], "application/pdf");
	assert_eq!(headers[hyper::header::CONTENT_DISPOSITION], "inline");
}

#[tokio::test]
async fn a_revoked_viewer_gets_forbidden_on_the_next_request() {
	let fake = Fake {
		page: Some(page("text/html", None)),
		revoked: true,
		served: Mutex::new(Vec::new()),
	};
	let response = dispatch(
		&fake,
		Some("label.aidashusercontent.app"),
		&hyper::Method::GET,
		"/",
		"",
		Some("__Host-aidash-preview=session"),
	)
	.await;
	let (status, headers, _) = body(response).await;
	assert_eq!(status, hyper::StatusCode::FORBIDDEN);
	assert_eq!(headers["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn a_request_without_the_session_cookie_is_forbidden() {
	let fake = Fake {
		page: Some(page("text/html", None)),
		revoked: false,
		served: Mutex::new(Vec::new()),
	};
	let response = dispatch(
		&fake,
		Some("label.aidashusercontent.app"),
		&hyper::Method::GET,
		"/",
		"",
		None,
	)
	.await;
	let (status, _, _) = body(response).await;
	assert_eq!(status, hyper::StatusCode::FORBIDDEN);
}

#[test]
fn the_egress_prefix_is_the_page_s_own_origin_path() {
	assert_eq!(EGRESS_PREFIX, "/__aidash/egress/");
	assert!(EGRESS_PREFIX.starts_with("/__aidash/"));
}

#[test]
fn a_forwarded_request_never_carries_the_viewer_s_cookies() {
	for name in [
		"cookie",
		"authorization",
		"host",
		"x-forwarded-for",
		"sec-fetch-site",
	] {
		assert!(!forwarded_request_header(name), "{name}");
	}
	for name in ["accept", "content-type", "range", "if-none-match"] {
		assert!(forwarded_request_header(name), "{name}");
	}
}

#[test]
fn a_pod_response_may_not_replace_the_confinement_headers() {
	for name in [
		"content-security-policy",
		"content-length",
		"connection",
		"transfer-encoding",
	] {
		assert!(!forwarded_response_header(name), "{name}");
	}
	assert!(forwarded_response_header("content-type"));
	assert!(forwarded_response_header("set-cookie"));
}
