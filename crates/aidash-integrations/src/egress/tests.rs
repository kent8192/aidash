use super::*;
use aidash_application::Result;
use aidash_domain::{egress::ExternalService, keyless::Destination};
use async_trait::async_trait;
use axum::{Router, body::Body as AxumBody, extract::State, routing::any};
use std::sync::{
	Mutex,
	atomic::{AtomicU64, Ordering},
};
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Default)]
struct Audit {
	sent: AtomicU64,
	received: AtomicU64,
	requests: Mutex<Vec<(String, String, String)>>,
	finished_records: Mutex<Vec<(u16, Option<String>)>>,
	finished: Notify,
}
#[async_trait]
impl Meter for Audit {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		match direction {
			Direction::Sent => &self.sent,
			Direction::Received => &self.received,
		}
		.fetch_add(bytes as u64, Ordering::Relaxed);
		Ok(())
	}
	async fn finish(&self, status: u16, _: u64, error: Option<&str>) -> Result<()> {
		self.finished_records
			.lock()
			.unwrap()
			.push((status, error.map(str::to_owned)));
		self.finished.notify_one();
		Ok(())
	}
}
struct FixtureAuthority {
	upstream: String,
	origin: String,
	audit: Arc<Audit>,
}
#[async_trait]
impl Authority for FixtureAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		if token != "operation-token" || service != "images" {
			return Err(aidash_application::Error::Forbidden);
		}
		self.audit
			.requests
			.lock()
			.unwrap()
			.push((method.into(), path.into(), digest.into()));
		Ok(Admission {
			gateway_origin: self.origin.clone(),
			destination: Destination::Permitted,
			request: Uuid::new_v4(),
			operation: Uuid::new_v4(),
			session: None,
			service: service.into(),
			definition: ExternalService {
				base_url: self.upstream.clone(),
				methods: vec!["GET".into(), "POST".into()],
				secret: Some("image-key".into()),
				authentication: Default::default(),
				limits: None,
			},
			expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
			credential: Some("canary-tenant-secret-never-returned".into()),
			route: None,
			meter: self.audit.clone(),
		})
	}
}

#[tokio::test]
async fn upload_and_download_stream_before_the_upload_finishes_and_audits_exclude_queries() {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route(
				"/{*path}",
				any(|request: Request<AxumBody>| async move {
					assert_eq!(
						request.headers()["authorization"],
						"Bearer canary-tenant-secret-never-returned"
					);
					assert!(request.headers().get("cookie").is_none());
					if request.uri().path() == "/redirect" {
						return Response::builder()
							.status(302)
							.header("location", "https://another-host.invalid/leak")
							.body(AxumBody::empty())
							.unwrap();
					}
					assert_eq!(request.headers()["content-type"], "image/png");
					Response::builder()
						.header("content-type", "image/png")
						.body(request.into_body())
						.unwrap()
				}),
			),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let (stop, receiver) = watch::channel(false);
	let authority = Arc::new(FixtureAuthority {
		upstream: upstream_url,
		origin: origin.clone(),
		audit: audit.clone(),
	});
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let release = Arc::new(Notify::new());
	let released = release.clone();
	let upload = async_stream::stream! {
		yield Ok::<_,std::io::Error>(Bytes::from_static(b"first-image-chunk"));
		released.notified().await;
		for _ in 0..160 { yield Ok(Bytes::from(vec![0x89;64*1024])); }
	};
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(15))
		.build()
		.unwrap();
	let response = tokio::time::timeout(
		Duration::from_secs(5),
		client
			.post(format!(
				"{origin}/v1/operation-token/images/upload?private=never-audit-this"
			))
			.header("content-type", "image/png")
			.header("authorization", "untrusted")
			.header("cookie", "dashboard=not-forwarded")
			.body(reqwest::Body::wrap_stream(upload))
			.send(),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	assert_eq!(response.headers()["content-type"], "image/png");
	let mut stream = response.bytes_stream();
	let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert_eq!(first, Bytes::from_static(b"first-image-chunk"));
	release.notify_one();
	let mut total = first.len();
	while let Some(bytes) = stream.next().await {
		total += bytes.unwrap().len();
	}
	assert_eq!(total, b"first-image-chunk".len() + 160 * 64 * 1024);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(audit.sent.load(Ordering::Relaxed), total as u64);
	assert_eq!(audit.received.load(Ordering::Relaxed), total as u64);
	assert_eq!(
		*audit.requests.lock().unwrap(),
		vec![(
			"POST".into(),
			"upload".into(),
			format!("{:x}", Sha256::digest(b"private=never-audit-this"))
		)]
	);
	let redirect = client
		.get(format!("{origin}/v1/operation-token/images/redirect"))
		.send()
		.await
		.unwrap();
	assert_eq!(redirect.status(), StatusCode::BAD_GATEWAY);
	assert!(
		redirect
			.text()
			.await
			.unwrap()
			.contains("egress_redirect_refused")
	);
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
	let _ = upstream_task.await;
}

struct DeadlineAuthority(FixtureAuthority);
struct StalledMeter(Arc<Audit>);
#[async_trait]
impl Meter for StalledMeter {
	async fn charge(&self, direction: Direction, _: usize) -> Result<()> {
		match direction {
			Direction::Sent => Ok(()),
			Direction::Received => std::future::pending().await,
		}
	}
	async fn finish(&self, status: u16, elapsed: u64, error: Option<&str>) -> Result<()> {
		self.0.finish(status, elapsed, error).await
	}
}
#[async_trait]
impl Authority for DeadlineAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		let mut admission = self.0.admit(token, service, method, path, digest).await?;
		admission.expires_at = chrono::Utc::now() + chrono::Duration::milliseconds(100);
		admission.meter = Arc::new(StalledMeter(self.0.audit.clone()));
		Ok(admission)
	}
}

#[tokio::test]
async fn a_stalled_budget_store_cannot_hold_a_response_past_the_operation_deadline() {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route("/image", any(|| async { "image" })),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(DeadlineAuthority(FixtureAuthority {
		upstream: upstream_url,
		origin: origin.clone(),
		audit: audit.clone(),
	}));
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/image"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	assert!(
		tokio::time::timeout(Duration::from_secs(1), response.bytes())
			.await
			.expect("the operation deadline closes a stalled stream")
			.is_err()
	);
	tokio::time::timeout(Duration::from_secs(1), audit.finished.notified())
		.await
		.unwrap();
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
	let _ = upstream_task.await;
}

#[test]
fn pypi_and_npm_indexes_rewrite_only_pinned_package_hosts_and_keep_integrity_data() {
	let index = Url::parse("https://pypi.org/simple/project/").unwrap();
	let html=br#"<a href="https://files.pythonhosted.org/packages/a.whl#sha256=abc" data-requires-python="&gt;=3">a</a>"#;
	let rewritten = String::from_utf8(
		rewrite_html(
			html,
			&index,
			"http://10.0.0.9:8950",
			"operation",
			"core.pypi",
		)
		.unwrap(),
	)
	.unwrap();
	assert!(rewritten.contains("http://10.0.0.9:8950/v1/operation/core.pypi/__asset/"));
	assert!(rewritten.contains("#sha256=abc"));
	assert!(rewritten.contains("data-requires-python=\"&gt;=3\""));
	let npm=br#"{"dist":{"tarball":"https://registry.npmjs.org/a/-/a.tgz","integrity":"sha512-fixed"},"url":"https://attacker.invalid/file"}"#;
	let json: serde_json::Value = serde_json::from_slice(
		&rewrite_json(
			npm,
			&Url::parse("https://registry.npmjs.org/a").unwrap(),
			"http://10.0.0.9:8950",
			"operation",
			"core.npm",
		)
		.unwrap(),
	)
	.unwrap();
	assert!(
		json["dist"]["tarball"]
			.as_str()
			.unwrap()
			.starts_with("http://10.0.0.9:8950/v1/operation/core.npm/__asset/")
	);
	assert_eq!(json["dist"]["integrity"], "sha512-fixed");
	assert_eq!(json["url"], "https://attacker.invalid/file");
	assert!(!package_url(
		"core.pypi",
		&Url::parse("https://files.pythonhosted.org.attacker.invalid/a").unwrap()
	));
}

#[derive(Default)]
struct Broker {
	/// The method, path and query, capability token and whether an
	/// `authorization` header reached the broker.
	calls: Mutex<Vec<(String, String, String, bool)>>,
}

async fn broker(
	State(state): State<Arc<Broker>>,
	request: Request<AxumBody>,
) -> Response<AxumBody> {
	let path = request.uri().path().to_owned();
	let query = request.uri().query().unwrap_or_default().to_owned();
	let token = request
		.headers()
		.get("x-aidash-secret-call")
		.and_then(|value| value.to_str().ok())
		.unwrap_or_default()
		.to_owned();
	let authorized = request.headers().get("authorization").is_some();
	state.calls.lock().unwrap().push((
		request.method().to_string(),
		format!("{path}?{query}"),
		token,
		authorized,
	));
	if let Some(code) = path.strip_prefix("/v1/forward/error/") {
		let status = match code {
			"secret_unavailable" => StatusCode::NOT_FOUND,
			"secret_destination_refused" => StatusCode::FORBIDDEN,
			"broker_rejected" => StatusCode::UNAUTHORIZED,
			"rate_limited" => StatusCode::TOO_MANY_REQUESTS,
			"upstream_unreachable" => StatusCode::BAD_GATEWAY,
			_ => StatusCode::SERVICE_UNAVAILABLE,
		};
		return Response::builder()
			.status(status)
			.header(BROKER_ERROR, code)
			.header("content-type", "application/json")
			.body(AxumBody::from(format!(
				"{{\"error\":\"{code}\",\"value\":\"canary-broker-secret\"}}"
			)))
			.unwrap();
	}
	if path == "/v1/forward/redirect" {
		return Response::builder()
			.status(302)
			.header("location", "https://api.example.test/v1/models/2?page=2")
			.body(AxumBody::empty())
			.unwrap();
	}
	if path == "/v1/forward/upload" {
		return Response::builder()
			.header("content-type", "application/octet-stream")
			.body(AxumBody::from_stream(
				request.into_body().into_data_stream(),
			))
			.unwrap();
	}
	Response::builder()
		.header("content-type", "application/json")
		.body(AxumBody::from(format!(
			"{{\"path\":\"{path}\",\"query\":\"{query}\"}}"
		)))
		.unwrap()
}

struct BrokerAuthority {
	broker: String,
	origin: String,
	audit: Arc<Audit>,
	definition: ExternalService,
}
#[async_trait]
impl Authority for BrokerAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		if token != "operation-token" || service != "images" {
			return Err(aidash_application::Error::Forbidden);
		}
		self.audit
			.requests
			.lock()
			.unwrap()
			.push((method.into(), path.into(), digest.into()));
		Ok(Admission {
			gateway_origin: self.origin.clone(),
			destination: Destination::PublicOnly,
			request: Uuid::new_v4(),
			operation: Uuid::new_v4(),
			session: None,
			service: service.into(),
			definition: self.definition.clone(),
			expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
			credential: None,
			route: Some(aidash_application::tenant_secrets::SecretRoute {
				base: Url::parse(&self.broker).unwrap(),
				token: if path == "invalid-token" {
					"invalid\ntoken"
				} else {
					"secret-call-token"
				}
				.into(),
			}),
			meter: self.audit.clone(),
		})
	}
}

fn secret_service() -> ExternalService {
	ExternalService {
		base_url: "https://api.example.test/v1/".into(),
		methods: vec!["GET".into(), "POST".into()],
		secret: Some("image-key".into()),
		authentication: Default::default(),
		limits: None,
	}
}

#[tokio::test]
async fn a_secret_bearing_service_streams_through_the_broker_and_rewrites_redirects() {
	let broker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let broker_url = format!("http://{}", broker_listener.local_addr().unwrap());
	let broker_state = Arc::new(Broker::default());
	let broker_state_for_server = broker_state.clone();
	let broker_task = tokio::spawn(async move {
		axum::serve(
			broker_listener,
			Router::new()
				.route("/{*path}", any(broker))
				.with_state(broker_state_for_server),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(BrokerAuthority {
		broker: format!("{broker_url}/v1/forward"),
		origin: origin.clone(),
		audit: audit.clone(),
		definition: secret_service(),
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(15))
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.unwrap();
	let get = client
		.get(format!("{origin}/v1/operation-token/images/models?q=1"))
		.header("authorization", "sandbox-supplied")
		.send()
		.await
		.unwrap();
	assert_eq!(get.status(), StatusCode::OK);
	let get_body = get.text().await.unwrap();
	assert!(get_body.contains("\"path\":\"/v1/forward/models\""));
	assert!(get_body.contains("\"query\":\"q=1\""));
	let release = Arc::new(Notify::new());
	let released = release.clone();
	let upload = async_stream::stream! {
		yield Ok::<_,std::io::Error>(Bytes::from_static(b"first-broker-chunk"));
		released.notified().await;
		for _ in 0..64 { yield Ok(Bytes::from(vec![0x42; 64 * 1024])); }
	};
	let response = tokio::time::timeout(
		Duration::from_secs(5),
		client
			.post(format!("{origin}/v1/operation-token/images/upload"))
			.header("content-type", "application/octet-stream")
			.body(reqwest::Body::wrap_stream(upload))
			.send(),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let mut stream = response.bytes_stream();
	let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert_eq!(first, Bytes::from_static(b"first-broker-chunk"));
	release.notify_one();
	let mut total = first.len();
	while let Some(bytes) = stream.next().await {
		total += bytes.unwrap().len();
	}
	assert_eq!(total, b"first-broker-chunk".len() + 64 * 64 * 1024);
	let redirect = client
		.get(format!("{origin}/v1/operation-token/images/redirect"))
		.send()
		.await
		.unwrap();
	assert_eq!(redirect.status(), StatusCode::FOUND);
	assert_eq!(
		redirect.headers()["location"],
		"/v1/operation-token/images/models/2?page=2"
	);
	// Both directions are debited through the gateway's own meter.
	assert_eq!(audit.sent.load(Ordering::Relaxed), total as u64);
	assert_eq!(
		audit.received.load(Ordering::Relaxed),
		(total + get_body.len()) as u64
	);
	{
		let calls = broker_state.calls.lock().unwrap();
		assert_eq!(calls.len(), 3);
		assert!(
			calls
				.iter()
				.all(|(_, _, token, authorized)| token == "secret-call-token" && !authorized)
		);
		assert_eq!(calls[0].1, "/v1/forward/models?q=1");
		assert_eq!(calls[1].1, "/v1/forward/upload?");
		assert_eq!(calls[2].1, "/v1/forward/redirect?");
	}
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	broker_task.abort();
	let _ = broker_task.await;
}

#[tokio::test]
async fn invalid_broker_capability_is_rejected_before_contacting_the_broker() {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(BrokerAuthority {
		broker: "http://127.0.0.1:9/v1/forward".into(),
		origin: origin.clone(),
		audit: audit.clone(),
		definition: secret_service(),
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/invalid-token"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
	assert_eq!(
		response.text().await.unwrap(),
		r#"{"error":"broker_unavailable"}"#
	);
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(503, Some("broker_unavailable".into()))]
	);
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
}

#[tokio::test]
async fn every_broker_error_code_is_reported_without_the_broker_body() {
	let broker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let broker_url = format!("http://{}", broker_listener.local_addr().unwrap());
	let broker_state = Arc::new(Broker::default());
	let broker_task = tokio::spawn(async move {
		axum::serve(
			broker_listener,
			Router::new()
				.route("/{*path}", any(broker))
				.with_state(broker_state),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(BrokerAuthority {
		broker: format!("{broker_url}/v1/forward"),
		origin: origin.clone(),
		audit: audit.clone(),
		definition: secret_service(),
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(15))
		.build()
		.unwrap();
	for (code, status) in [
		("secret_unavailable", StatusCode::FAILED_DEPENDENCY),
		("secret_destination_refused", StatusCode::FORBIDDEN),
		("broker_rejected", StatusCode::BAD_GATEWAY),
		("broker_unavailable", StatusCode::SERVICE_UNAVAILABLE),
		("rate_limited", StatusCode::TOO_MANY_REQUESTS),
		("upstream_unreachable", StatusCode::BAD_GATEWAY),
	] {
		let response = client
			.get(format!("{origin}/v1/operation-token/images/error/{code}"))
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), status, "{code}");
		let body = response.text().await.unwrap();
		assert_eq!(body, format!("{{\"error\":\"{code}\"}}"), "{code}");
		assert!(!body.contains("canary"), "{code}");
	}
	{
		let finished = audit.finished_records.lock().unwrap();
		assert_eq!(finished.len(), 6);
		for (code, status) in [
			("secret_unavailable", StatusCode::FAILED_DEPENDENCY),
			("secret_destination_refused", StatusCode::FORBIDDEN),
			("broker_rejected", StatusCode::BAD_GATEWAY),
			("broker_unavailable", StatusCode::SERVICE_UNAVAILABLE),
			("rate_limited", StatusCode::TOO_MANY_REQUESTS),
			("upstream_unreachable", StatusCode::BAD_GATEWAY),
		] {
			assert!(finished.contains(&(status.as_u16(), Some(code.to_owned()))));
		}
	}
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	broker_task.abort();
	let _ = broker_task.await;
}

struct DirectAuthority {
	upstream: String,
	origin: String,
	audit: Arc<Audit>,
}
#[async_trait]
impl Authority for DirectAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		if token != "operation-token" || service != "images" {
			return Err(aidash_application::Error::Forbidden);
		}
		self.audit
			.requests
			.lock()
			.unwrap()
			.push((method.into(), path.into(), digest.into()));
		Ok(Admission {
			gateway_origin: self.origin.clone(),
			destination: Destination::Permitted,
			request: Uuid::new_v4(),
			operation: Uuid::new_v4(),
			session: None,
			service: service.into(),
			definition: ExternalService {
				base_url: self.upstream.clone(),
				methods: vec!["GET".into()],
				secret: None,
				authentication: Default::default(),
				limits: None,
			},
			expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
			credential: None,
			route: None,
			meter: self.audit.clone(),
		})
	}
}

#[tokio::test]
async fn a_service_without_a_secret_still_goes_direct() {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route(
				"/{*path}",
				any(|request: Request<AxumBody>| async move {
					assert!(request.headers().get("x-aidash-secret-call").is_none());
					Response::builder()
						.status(StatusCode::IM_A_TEAPOT)
						.header("content-type", "text/plain")
						.header(BROKER_ERROR, "broker_rejected")
						.body(AxumBody::from("direct"))
						.unwrap()
				}),
			),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(DirectAuthority {
		upstream: upstream_url,
		origin: origin.clone(),
		audit: audit.clone(),
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(listener, authority, receiver));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/plain"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
	assert_eq!(response.text().await.unwrap(), "direct");
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	{
		let records = audit.finished_records.lock().unwrap();
		assert_eq!(records.len(), 1);
		assert_eq!(records[0].0, 418);
		assert_ne!(records[0].1.as_deref(), Some("broker_rejected"));
	}
	assert_eq!(audit.sent.load(Ordering::Relaxed), 0);
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
	let _ = upstream_task.await;
}

#[test]
fn a_brokered_request_keeps_the_service_path_exactly() {
	let route = aidash_application::tenant_secrets::SecretRoute {
		base: Url::parse("https://broker.test/v1/forward").unwrap(),
		token: "secret-call-token".into(),
	};
	for (base, service, forwarded) in [
		// An endpoint-style base forwards its last segment without a slash.
		(
			"https://api.example.test/invoke",
			"https://api.example.test/invoke",
			"https://broker.test/v1/forward/invoke",
		),
		(
			"https://api.example.test/v1/invoke",
			"https://api.example.test/v1/invoke?x=1",
			"https://broker.test/v1/forward/invoke?x=1",
		),
		// A directory-style base forwards what follows it.
		(
			"https://api.example.test/v1/",
			"https://api.example.test/v1/",
			"https://broker.test/v1/forward",
		),
		(
			"https://api.example.test/v1/",
			"https://api.example.test/v1/items/",
			"https://broker.test/v1/forward/items/",
		),
	] {
		let url = broker_url(
			&route,
			&Url::parse(service).unwrap(),
			&Url::parse(base).unwrap(),
		)
		.unwrap();
		assert_eq!(url.as_str(), forwarded, "{base} {service}");
	}
}
