use super::*;
use aidash_application::Result;
use aidash_domain::{egress::ExternalService, keyless::Destination};
use async_trait::async_trait;
use axum::{Router, body::Body as AxumBody, extract::State, routing::any};
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::Notify;
use uuid::Uuid;

#[test]
fn inactive_operation_is_forbidden_with_its_own_code() {
	let refusal = Refusal::of(&aidash_application::Error::Conflict(
		aidash_domain::egress::OPERATION_INACTIVE.into(),
	));
	assert_eq!(refusal.status, StatusCode::FORBIDDEN);
	assert_eq!(refusal.code, "operation_inactive");
}

#[derive(Default)]
struct Audit {
	stopped: AtomicBool,
	stall_charge: AtomicBool,
	slow_checks: AtomicU64,
	failed_checks: AtomicU64,
	check_started: Notify,
	sent: AtomicU64,
	received: AtomicU64,
	charges: AtomicU64,
	requests: Mutex<Vec<(String, String, String)>>,
	finished_records: Mutex<Vec<(u16, Option<String>)>>,
	times: Mutex<Vec<RequestTimes>>,
	finished: Notify,
}
#[async_trait]
impl Meter for Audit {
	async fn check_active(&self) -> Result<()> {
		self.check_started.notify_one();
		if self
			.slow_checks
			.fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
				remaining.checked_sub(1)
			})
			.is_ok()
		{
			tokio::time::sleep(Duration::from_secs(2)).await;
		}
		if self
			.failed_checks
			.fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
				remaining.checked_sub(1)
			})
			.is_ok()
		{
			return Err(aidash_application::Error::External(
				"authority unavailable".into(),
			));
		}
		if self.stopped.load(Ordering::Acquire) {
			return Err(aidash_application::Error::Conflict(
				aidash_domain::egress::OPERATION_EXPIRED.into(),
			));
		}
		Ok(())
	}
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		if self.stall_charge.load(Ordering::Acquire) {
			std::future::pending::<()>().await;
		}
		if self.stopped.load(Ordering::Acquire) {
			return Err(aidash_application::Error::Conflict(
				aidash_domain::egress::OPERATION_EXPIRED.into(),
			));
		}
		self.charges.fetch_add(1, Ordering::Relaxed);
		match direction {
			Direction::Sent => &self.sent,
			Direction::Received => &self.received,
		}
		.fetch_add(bytes as u64, Ordering::Relaxed);
		Ok(())
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		self.times.lock().unwrap().push(times);
		self.finished_records
			.lock()
			.unwrap()
			.push((status, error.map(str::to_owned)));
		self.finished.notify_one();
		Ok(())
	}
}

async fn idle_monitor_fixture(
	audit: Arc<Audit>,
) -> (
	reqwest::Response,
	watch::Sender<bool>,
	tokio::task::JoinHandle<crate::Result<()>>,
	tokio::task::JoinHandle<()>,
) {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route(
				"/{*path}",
				any(|| async {
					let stream = async_stream::stream! {
						yield Ok::<_, std::io::Error>(Bytes::from_static(b"started"));
						std::future::pending::<()>().await;
					};
					AxumBody::from_stream(stream)
				}),
			),
		)
		.await
		.unwrap();
	});
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(serve(
		listener,
		Arc::new(FixtureAuthority {
			upstream: upstream_url,
			origin: origin.clone(),
			audit,
		}),
		receiver,
	));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/idle"))
		.await
		.unwrap();
	(response, stop, gateway, upstream_task)
}

#[tokio::test]
async fn a_slow_authority_check_does_not_cut_a_healthy_stream() {
	let audit = Arc::new(Audit::default());
	audit.slow_checks.store(1, Ordering::Release);
	let (response, stop, gateway, upstream) = idle_monitor_fixture(audit.clone()).await;
	let mut body = response.bytes_stream();
	assert_eq!(
		body.next().await.unwrap().unwrap(),
		Bytes::from_static(b"started")
	);
	tokio::time::timeout(Duration::from_secs(6), audit.check_started.notified())
		.await
		.unwrap();
	tokio::time::sleep(Duration::from_millis(1200)).await;
	assert!(
		tokio::time::timeout(Duration::from_millis(200), body.next())
			.await
			.is_err(),
		"one slow authority check must not cut the stream"
	);
	audit.stopped.store(true, Ordering::Release);
	let next = tokio::time::timeout(Duration::from_secs(6), body.next())
		.await
		.expect("revocation did not stop the stream");
	assert!(matches!(next, Some(Err(_))));
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream.abort();
}

#[tokio::test]
async fn three_consecutive_authority_failures_cut_the_stream() {
	let audit = Arc::new(Audit::default());
	audit.failed_checks.store(3, Ordering::Release);
	let (response, stop, gateway, upstream) = idle_monitor_fixture(audit.clone()).await;
	let mut body = response.bytes_stream();
	assert_eq!(
		body.next().await.unwrap().unwrap(),
		Bytes::from_static(b"started")
	);
	tokio::time::timeout(Duration::from_secs(6), audit.check_started.notified())
		.await
		.unwrap();
	assert!(
		tokio::time::timeout(Duration::from_secs(1), body.next())
			.await
			.is_err(),
		"the first authority failure must not cut the stream"
	);
	let next = tokio::time::timeout(Duration::from_secs(10), body.next())
		.await
		.expect("three failures did not stop the stream");
	assert!(matches!(next, Some(Err(_))));
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream.abort();
}

#[tokio::test]
async fn a_stopped_operation_interrupts_an_idle_response() {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route(
				"/{*path}",
				any(|| async {
					let stream = async_stream::stream! {
						yield Ok::<_, std::io::Error>(Bytes::from_static(b"started"));
						std::future::pending::<()>().await;
					};
					AxumBody::from_stream(stream)
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
	let gateway = tokio::spawn(serve(
		listener,
		Arc::new(FixtureAuthority {
			upstream: upstream_url,
			origin: origin.clone(),
			audit: audit.clone(),
		}),
		receiver,
	));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/idle"))
		.await
		.unwrap();
	let mut body = response.bytes_stream();
	assert_eq!(
		body.next().await.unwrap().unwrap(),
		Bytes::from_static(b"started")
	);
	audit.stall_charge.store(true, Ordering::Release);
	audit.stopped.store(true, Ordering::Release);
	let next = tokio::time::timeout(Duration::from_secs(6), body.next())
		.await
		.expect("idle response was not interrupted after stop");
	assert!(matches!(next, Some(Err(_))), "expected an interrupted body");
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
}
#[tokio::test]
async fn a_stopped_operation_releases_an_unread_response() {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new().route(
				"/{*path}",
				any(|| async {
					let stream = futures_util::stream::repeat_with(|| {
						Ok::<_, std::io::Error>(Bytes::from(vec![7; 64 * 1024]))
					});
					AxumBody::from_stream(stream)
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
	let gateway = tokio::spawn(serve(
		listener,
		Arc::new(FixtureAuthority {
			upstream: upstream_url,
			origin: origin.clone(),
			audit: audit.clone(),
		}),
		receiver,
	));
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/large"))
		.await
		.unwrap();
	// Let the unread client fill its network buffers and stall the response.
	tokio::time::sleep(Duration::from_millis(500)).await;
	audit.stopped.store(true, Ordering::Release);
	tokio::time::timeout(Duration::from_secs(6), audit.finished.notified())
		.await
		.expect("the unread response retained its upstream after stop");
	// The client never reads, so the stop is seen by the bridge and not by the
	// body stream. The record still names the stop, as the client's error does.
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(200, Some("operation_expired".into()))]
	);
	drop(response);
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
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
			operation: Some(Uuid::new_v4()),
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
	assert!(
		audit.charges.load(Ordering::Relaxed) <= 24,
		"body chunks must be batched in both directions"
	);
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
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		self.0.finish(status, times, error).await
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
			// Large enough for a batch debit, which then stalls mid-stream.
			Router::new().route("/image", any(|| async { vec![0x89u8; 2 << 20] })),
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
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(200, Some("operation_expired".into()))]
	);
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
			operation: Some(Uuid::new_v4()),
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
	let observed = Observed::start();
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
	// Failures to reach the service, or of the broker, warn with their code; a
	// decision about the Tenant's request does not.
	let logs = observed.logs();
	assert_eq!(logs.matches("Egress upstream failed").count(), 3, "{logs}");
	for code in [
		"upstream_unreachable",
		"broker_rejected",
		"broker_unavailable",
	] {
		assert!(logs.contains(code), "{code} in {logs}");
	}
	for private in ["canary", "secret_unavailable", "rate_limited"] {
		assert!(!logs.contains(private), "{private} in {logs}");
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
			operation: Some(Uuid::new_v4()),
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

struct ScopeServiceFixture {
	scopes: Arc<aidash_application::egress::test_support::MemoryScopes>,
	upstream: String,
	origin: String,
	audit: Arc<Audit>,
}
#[async_trait]
impl aidash_application::egress::EgressScopes for ScopeServiceFixture {
	async fn resolve(&self, credential: &str) -> Result<aidash_application::egress::ScopeSettings> {
		self.scopes.resolve(credential).await
	}
	async fn admit_external(
		&self,
		credential: &str,
		service: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<Admission> {
		let scope = self.scopes.resolve(credential).await?;
		self.scopes.check_active(&scope, credential).await?;
		if service != "images" {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(Admission {
			gateway_origin: self.origin.clone(),
			destination: Destination::Permitted,
			request: Uuid::new_v4(),
			operation: None,
			session: None,
			service: service.into(),
			definition: ExternalService {
				base_url: self.upstream.clone(),
				methods: vec!["GET".into()],
				secret: Some("image-key".into()),
				authentication: Default::default(),
				limits: None,
			},
			expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
			credential: Some("canary-tenant-secret".into()),
			route: None,
			meter: self.audit.clone(),
		})
	}
	async fn admit_connection(
		&self,
		scope: &aidash_application::egress::ScopeSettings,
	) -> Result<()> {
		self.scopes.admit_connection(scope).await
	}
	async fn check_active(
		&self,
		scope: &aidash_application::egress::ScopeSettings,
		credential: &str,
	) -> Result<aidash_application::egress::ScopeSettings> {
		self.scopes.check_active(scope, credential).await
	}
	async fn debit(
		&self,
		scope: &aidash_application::egress::ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: chrono::NaiveDate,
	) -> Result<()> {
		self.scopes.debit(scope, direction, bytes, day).await
	}
	async fn refund(
		&self,
		scope: &aidash_application::egress::ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: chrono::NaiveDate,
	) -> Result<()> {
		self.scopes.refund(scope, direction, bytes, day).await
	}
	async fn record(&self, record: aidash_application::egress::ConnectionRecord) -> Result<()> {
		self.scopes.record(record).await
	}
	async fn record_refusals(
		&self,
		tallies: Vec<aidash_application::egress::RefusalTally>,
	) -> Result<()> {
		self.scopes.record_refusals(tallies).await
	}
	async fn notify_many_hosts(
		&self,
		scope: &aidash_domain::egress::ScopeId,
		hosts: usize,
	) -> Result<()> {
		self.scopes.notify_many_hosts(scope, hosts).await
	}
}

#[tokio::test]
async fn external_service_accepts_scope_credential_without_forwarding_it() {
	use aidash_application::egress::{ScopeSettings, test_support::MemoryScopes};
	use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget, ScopeId};
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
						"Bearer canary-tenant-secret"
					);
					assert!(request.headers().get("proxy-authorization").is_none());
					if request.uri().path() == "/redirect" {
						return Response::builder()
							.status(StatusCode::FOUND)
							.header("location", "/next")
							.body(AxumBody::empty())
							.unwrap();
					}
					Response::new(AxumBody::from("ok"))
				}),
			),
		)
		.await
		.unwrap();
	});
	let settings = ScopeSettings {
		id: ScopeId {
			tenant: "tenant".into(),
			node: "node".into(),
			scratch_space: Uuid::new_v4(),
		},
		allowed_hosts: AllowedHosts::default(),
		budget: DailyBudget::default(),
		anomaly: AnomalyRule::default(),
		operator_allows_all: false,
		tunnel_limits: Default::default(),
	};
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(FixtureAuthority {
		upstream: upstream_url.clone(),
		origin: origin.clone(),
		audit: audit.clone(),
	});
	let scopes = Arc::new(ScopeServiceFixture {
		scopes: Arc::new(MemoryScopes::new("scope-token".into(), settings)),
		upstream: upstream_url,
		origin: origin.clone(),
		audit,
	});
	let (stop, stopping) = watch::channel(false);
	let gateway = tokio::spawn(serve_with_scopes(
		listener,
		authority,
		Some(scopes),
		ProxyLimits::default(),
		stopping,
	));
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.unwrap();
	let url = format!("{origin}/v2/services/images/photo?private=yes");
	assert_eq!(
		client
			.get(&url)
			.bearer_auth("wrong")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::FORBIDDEN
	);
	let response = client
		.get(&url)
		.bearer_auth("scope-token")
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	assert_eq!(response.text().await.unwrap(), "ok");
	let redirect = client
		.get(format!("{origin}/v2/services/images/redirect"))
		.bearer_auth("scope-token")
		.send()
		.await
		.unwrap();
	assert_eq!(redirect.status(), StatusCode::FOUND);
	assert_eq!(redirect.headers()["location"], "/v2/services/images/next");
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	upstream_task.abort();
}

/// Commits every debit like the server meter, then refuses once a counter
/// reaches `limit` or when the operation has expired.
struct BudgetMeter {
	audit: Arc<Audit>,
	limit: u64,
	expired: bool,
}
#[async_trait]
impl Meter for BudgetMeter {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		self.audit.charge(direction, bytes).await?;
		let used = match direction {
			Direction::Sent => &self.audit.sent,
			Direction::Received => &self.audit.received,
		}
		.load(Ordering::Relaxed);
		if self.expired {
			return Err(aidash_application::Error::Conflict(
				aidash_domain::egress::OPERATION_EXPIRED.into(),
			));
		}
		if self.limit == 0 {
			return Err(aidash_application::Error::Refused(
				"egress_budget_exhausted".into(),
			));
		}
		if used >= self.limit {
			return Err(aidash_application::Error::Conflict(
				aidash_domain::egress::LIMIT_EXCEEDED.into(),
			));
		}
		Ok(())
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		self.audit.finish(status, times, error).await
	}
}
struct MeterAuthority {
	inner: FixtureAuthority,
	limit: u64,
	expired: bool,
	admission: Duration,
}
#[async_trait]
impl Authority for MeterAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		tokio::time::sleep(self.admission).await;
		let mut admission = self
			.inner
			.admit(token, service, method, path, digest)
			.await?;
		admission.meter = Arc::new(BudgetMeter {
			audit: self.inner.audit.clone(),
			limit: self.limit,
			expired: self.expired,
		});
		Ok(admission)
	}
}

const CHUNK: usize = 64 * 1024;

/// A gateway in front of [`fixture_upstream`] with a [`BudgetMeter`].
async fn metered_gateway(
	limit: u64,
	expired: bool,
) -> (
	String,
	Arc<Audit>,
	Vec<tokio::task::JoinHandle<()>>,
	watch::Sender<bool>,
) {
	gateway_admitting_after(limit, expired, Duration::ZERO).await
}

/// An upstream whose `/download/{n}` streams `n` 64 KiB chunks, `/sized/{n}`
/// streams them with a `Content-Length`, `/broken` streams 40 chunks and then
/// fails, and `/upload` drains the request body before it answers.
async fn fixture_upstream() -> (String, tokio::task::JoinHandle<()>) {
	let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_url = format!("http://{}/", upstream.local_addr().unwrap());
	let upstream_task = tokio::spawn(async move {
		axum::serve(
			upstream,
			Router::new()
				.route(
					"/download/{n}",
					any(
						|axum::extract::Path(n): axum::extract::Path<usize>| async move {
							let chunks = futures_util::stream::iter(
								(0..n)
									.map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![7; CHUNK]))),
							);
							AxumBody::from_stream(chunks)
						},
					),
				)
				.route(
					"/sized/{n}",
					any(
						|axum::extract::Path(n): axum::extract::Path<usize>| async move {
							let chunks = futures_util::stream::iter(
								(0..n)
									.map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![7; CHUNK]))),
							);
							Response::builder()
								.header("content-length", n * CHUNK)
								.body(AxumBody::from_stream(chunks))
								.unwrap()
						},
					),
				)
				.route(
					"/broken",
					any(|| async {
						let chunks = async_stream::stream! {
							for _ in 0..40 { yield Ok(Bytes::from(vec![7; CHUNK])); }
							// Let the client read every chunk before the failure.
							tokio::time::sleep(Duration::from_millis(300)).await;
							yield Err(std::io::Error::other("upstream broke"));
						};
						AxumBody::from_stream(chunks)
					}),
				)
				.route(
					"/upload",
					any(|request: Request<AxumBody>| async move {
						let body = axum::body::to_bytes(request.into_body(), usize::MAX)
							.await
							.map(|bytes| bytes.len())
							.unwrap_or_default();
						body.to_string()
					}),
				),
		)
		.await
		.unwrap();
	});
	(upstream_url, upstream_task)
}

/// [`metered_gateway`] whose authority takes `admission` to admit a request.
async fn gateway_admitting_after(
	limit: u64,
	expired: bool,
	admission: Duration,
) -> (
	String,
	Arc<Audit>,
	Vec<tokio::task::JoinHandle<()>>,
	watch::Sender<bool>,
) {
	let (upstream_url, upstream_task) = fixture_upstream().await;
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let audit = Arc::new(Audit::default());
	let authority = Arc::new(MeterAuthority {
		inner: FixtureAuthority {
			upstream: upstream_url,
			origin: origin.clone(),
			audit: audit.clone(),
		},
		limit,
		expired,
		admission,
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(async move { serve(listener, authority, receiver).await.unwrap() });
	(origin, audit, vec![upstream_task, gateway], stop)
}

/// Admits like [`FixtureAuthority`], with one meter shared by every request.
struct FixedMeter {
	inner: FixtureAuthority,
	meter: Arc<dyn Meter>,
}
#[async_trait]
impl Authority for FixedMeter {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<Admission> {
		let mut admission = self
			.inner
			.admit(token, service, method, path, digest)
			.await?;
		admission.meter = self.meter.clone();
		Ok(admission)
	}
}
/// A gateway in front of [`fixture_upstream`] whose requests `meter` meters.
async fn gateway_metered_by(
	meter: Arc<dyn Meter>,
) -> (
	String,
	Vec<tokio::task::JoinHandle<()>>,
	watch::Sender<bool>,
) {
	let (upstream_url, upstream_task) = fixture_upstream().await;
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let authority = Arc::new(FixedMeter {
		inner: FixtureAuthority {
			upstream: upstream_url,
			origin: origin.clone(),
			audit: Arc::new(Audit::default()),
		},
		meter,
	});
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(async move { serve(listener, authority, receiver).await.unwrap() });
	(origin, vec![upstream_task, gateway], stop)
}

async fn shutdown(tasks: Vec<tokio::task::JoinHandle<()>>, stop: watch::Sender<bool>) {
	stop.send(true).unwrap();
	for task in tasks {
		task.abort();
		let _ = task.await;
	}
}

/// Reads a response body until it ends or fails; returns the bytes read and
/// whether it ended cleanly.
async fn drain(response: reqwest::Response) -> (usize, bool) {
	let mut stream = response.bytes_stream();
	let mut total = 0;
	while let Some(next) = stream.next().await {
		match next {
			Ok(bytes) => total += bytes.len(),
			Err(_) => return (total, false),
		}
	}
	(total, true)
}

#[tokio::test]
async fn a_large_download_is_debited_in_batches_and_totals_the_streamed_bytes() {
	let (origin, audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/download/160"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let (total, clean) = drain(response).await;
	assert!(clean);
	assert_eq!(total, 160 * CHUNK);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(audit.received.load(Ordering::Relaxed), total as u64);
	// 10 MiB in 1 MiB batches, not one debit per chunk.
	assert!(audit.charges.load(Ordering::Relaxed) <= 11);
	assert_eq!(*audit.finished_records.lock().unwrap(), vec![(200, None)]);
	shutdown(tasks, stop).await;
}

/// The authority's recheck happens before `duration_ms` starts, which is how it
/// stayed hidden while it grew with a Run's reads (#752).
#[tokio::test]
async fn a_request_records_how_long_its_admission_took() {
	let admission = Duration::from_millis(50);
	let (origin, audit, tasks, stop) = gateway_admitting_after(u64::MAX, false, admission).await;
	for path in ["download/1", "sized/1"] {
		let response = reqwest::get(format!("{origin}/v1/operation-token/images/{path}"))
			.await
			.unwrap();
		drain(response).await;
		tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
			.await
			.unwrap();
	}
	let times = audit.times.lock().unwrap().clone();
	assert_eq!(times.len(), 2);
	assert!(
		times
			.iter()
			.all(|times| times.admission_ms >= millis(admission)),
		"{times:?}"
	);
	shutdown(tasks, stop).await;
}

/// Hyper stops polling a body once `Content-Length` bytes are written, so this
/// path never reaches the end of the stream (#710).
#[tokio::test]
async fn a_download_with_a_content_length_is_recorded_as_complete() {
	let (origin, audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/sized/40"))
		.await
		.unwrap();
	assert_eq!(response.content_length(), Some(40 * CHUNK as u64));
	let (total, clean) = drain(response).await;
	assert!(clean);
	assert_eq!(total, 40 * CHUNK);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(audit.received.load(Ordering::Relaxed), total as u64);
	assert_eq!(*audit.finished_records.lock().unwrap(), vec![(200, None)]);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn a_download_the_client_abandons_is_recorded_as_interrupted() {
	let (origin, audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	// Far more than the socket buffers hold, so the gateway is still writing.
	let mut response = reqwest::get(format!("{origin}/v1/operation-token/images/sized/1600"))
		.await
		.unwrap();
	response.chunk().await.unwrap().unwrap();
	drop(response);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(200, Some("stream_interrupted".into()))]
	);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn an_interrupted_download_still_debits_every_streamed_byte() {
	let (origin, audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/broken"))
		.await
		.unwrap();
	let (total, clean) = drain(response).await;
	assert!(!clean);
	assert_eq!(total, 40 * CHUNK);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(audit.received.load(Ordering::Relaxed), total as u64);
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(200, Some("upstream_interrupted".into()))]
	);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn an_exhausted_budget_stops_a_download_after_at_most_one_batch_over() {
	let limit = 2 * BATCH_BYTES + 1;
	let (origin, audit, tasks, stop) = metered_gateway(limit, false).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/download/160"))
		.await
		.unwrap();
	let (total, clean) = drain(response).await;
	assert!(!clean, "the refused debit ends the body early");
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	let received = audit.received.load(Ordering::Relaxed);
	assert!(received >= limit);
	assert!(received <= limit + BATCH_BYTES + CHUNK as u64);
	assert!(total as u64 <= received);
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(200, Some("egress_limit_exceeded".into()))]
	);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn an_exhausted_upload_budget_is_reported_as_the_limit_not_the_upstream() {
	let (origin, audit, tasks, stop) = metered_gateway(BATCH_BYTES, false).await;
	// Reach the limit without leaving additional client bytes to race the
	// gateway's refusal and reset the socket before its response is read.
	let upload = futures_util::stream::iter(std::iter::once(Ok::<_, std::io::Error>(Bytes::from(
		vec![1; BATCH_BYTES as usize],
	))));
	let response = reqwest::Client::new()
		.post(format!("{origin}/v1/operation-token/images/upload"))
		.body(reqwest::Body::wrap_stream(upload))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
	assert_eq!(
		response.text().await.unwrap(),
		r#"{"error":"egress_limit_exceeded"}"#
	);
	let sent = audit.sent.load(Ordering::Relaxed);
	assert!((BATCH_BYTES..=2 * BATCH_BYTES).contains(&sent));
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(429, Some("egress_limit_exceeded".into()))]
	);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn scoped_refusals_are_reported_on_upload() {
	for (limit, status, code) in [(0, StatusCode::TOO_MANY_REQUESTS, "egress_budget_exhausted")] {
		let (origin, audit, tasks, stop) = metered_gateway(limit, false).await;
		let upload = futures_util::stream::iter(
			(0..32).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![1; CHUNK]))),
		);
		let response = reqwest::Client::new()
			.post(format!("{origin}/v1/operation-token/images/upload"))
			.body(reqwest::Body::wrap_stream(upload))
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), status);
		assert_eq!(
			response.text().await.unwrap(),
			format!(r#"{{"error":"{code}"}}"#)
		);
		assert_eq!(
			*audit.finished_records.lock().unwrap(),
			vec![(status.as_u16(), Some(code.into()))]
		);
		shutdown(tasks, stop).await;
	}
}

#[tokio::test]
async fn an_expired_operation_is_reported_as_expired_not_as_an_exhausted_budget() {
	let (origin, audit, tasks, stop) = metered_gateway(u64::MAX, true).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/download/1"))
		.await
		.unwrap();
	let (_, clean) = drain(response).await;
	assert!(!clean);
	tokio::time::timeout(Duration::from_secs(5), audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(audit.received.load(Ordering::Relaxed), CHUNK as u64);
	// Avoid leaving an unread multi-chunk body that can reset the client
	// before it receives the gateway's expired-operation response.
	let upload = futures_util::stream::iter(std::iter::once(Ok::<_, std::io::Error>(Bytes::from(
		vec![1; CHUNK],
	))));
	let response = reqwest::Client::new()
		.post(format!("{origin}/v1/operation-token/images/upload"))
		.body(reqwest::Body::wrap_stream(upload))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::FORBIDDEN);
	assert_eq!(
		response.text().await.unwrap(),
		r#"{"error":"operation_expired"}"#
	);
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![
			(200, Some("operation_expired".into())),
			(403, Some("operation_expired".into()))
		]
	);
	shutdown(tasks, stop).await;
}

#[test]
fn a_rewritten_package_index_never_forwards_the_clients_accept_encoding() {
	// npm and uv always send Accept-Encoding. Forwarding it beside the gateway's
	// `identity` made registry.npmjs.org answer with gzip, which the JSON
	// rewrite cannot read (#679).
	assert!(!request_header_forwarded("accept-encoding", &[], true));
	assert!(request_header_forwarded("accept-encoding", &[], false));
	assert!(request_header_forwarded("accept", &[], true));
	assert!(!request_header_forwarded("cookie", &[], false));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_client_write_times_out() {
	use tokio::io::AsyncWriteExt;
	let (socket, _client) = tokio::io::duplex(1);
	let task = tokio::spawn(async move {
		let mut socket = WriteDeadline::new(socket, Duration::from_secs(2));
		socket.write_all(b"ab").await
	});
	tokio::task::yield_now().await;
	tokio::time::advance(Duration::from_secs(3)).await;
	let error = task.await.unwrap().unwrap_err();
	assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

/// Sends a whole request before reading anything, as a client without a
/// concurrent reader does, so a reset cannot be mistaken for a slow answer.
async fn upload_then_read(origin: &str, path: &str, chunks: usize) -> std::io::Result<String> {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let mut socket = tokio::net::TcpStream::connect(origin.trim_start_matches("http://")).await?;
	let length = chunks * CHUNK;
	socket
		.write_all(
			format!("POST {path} HTTP/1.1\r\nhost: gateway\r\ncontent-length: {length}\r\n\r\n")
				.as_bytes(),
		)
		.await?;
	for _ in 0..chunks {
		socket.write_all(&[1; CHUNK]).await?;
	}
	let mut answer = Vec::new();
	socket.read_to_end(&mut answer).await?;
	Ok(String::from_utf8_lossy(&answer).into_owned())
}

#[tokio::test]
async fn a_refusal_during_an_upload_reaches_a_client_that_sends_everything_first() {
	let (origin, audit, tasks, stop) = metered_gateway(BATCH_BYTES, false).await;
	let answer = tokio::time::timeout(
		Duration::from_secs(15),
		upload_then_read(&origin, "/v1/operation-token/images/upload", 64),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(answer.starts_with("HTTP/1.1 429"), "{answer}");
	assert!(answer.contains("egress_limit_exceeded"), "{answer}");
	let sent = audit.sent.load(Ordering::Relaxed);
	assert!(
		sent <= 2 * BATCH_BYTES,
		"discarded bytes are not charged: {sent}"
	);
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn a_refusal_at_admission_reaches_a_client_that_sends_everything_first() {
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let answer = tokio::time::timeout(
		Duration::from_secs(15),
		upload_then_read(&origin, "/v1/wrong-token/images/upload", 64),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
	assert!(answer.contains("egress_forbidden"), "{answer}");
	shutdown(tasks, stop).await;
}

#[tokio::test]
async fn a_refusal_for_full_slots_reaches_a_client_that_sends_everything_first() {
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let address = origin.trim_start_matches("http://").to_owned();
	let mut held = Vec::new();
	for _ in 0..CONNECTION_SLOTS {
		held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
	}
	let answer = tokio::time::timeout(
		Duration::from_secs(15),
		upload_then_read(&origin, "/v1/operation-token/images/upload", 64),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
	assert!(answer.contains(CONNECTION_LIMIT), "{answer}");
	drop(held);
	shutdown(tasks, stop).await;
}

#[test]
fn only_a_request_with_a_body_lingers() {
	let unread = UnreadBody::default();
	let request = |method, body: &'static [u8]| {
		Request::builder()
			.method(method)
			.body(Full::new(Bytes::from_static(body)))
			.unwrap()
	};
	unread.note(&request(hyper::Method::POST, b"body"));
	assert!(unread.0.load(Ordering::Relaxed));
	unread.note(&request(hyper::Method::GET, b""));
	assert!(!unread.0.load(Ordering::Relaxed));
	unread.note(&request(hyper::Method::CONNECT, b"body"));
	assert!(!unread.0.load(Ordering::Relaxed));
}

/// A tunnel closes its side toward the client when the upstream stops sending,
/// through the same socket wrapper, and what the client still sends must arrive.
#[tokio::test]
async fn an_upgraded_connection_keeps_reading_after_closing_its_sending_side() {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let (tunnel, tunneled) = tokio::sync::oneshot::channel();
	let gateway = tokio::spawn(async move {
		let (socket, _) = listener.accept().await.unwrap();
		let unread = UnreadBody::default();
		let noting = unread.clone();
		let tunnel = Arc::new(Mutex::new(Some(tunnel)));
		let service = service_fn(move |mut request: Request<hyper::body::Incoming>| {
			noting.note(&request);
			let upgrade = hyper::upgrade::on(&mut request);
			let tunnel = tunnel.lock().unwrap().take().unwrap();
			tokio::spawn(async move {
				let mut client = TokioIo::new(upgrade.await.unwrap());
				client.shutdown().await.unwrap();
				let mut input = [0; 5];
				client.read_exact(&mut input).await.unwrap();
				tunnel.send(input).unwrap();
			});
			async { Ok::<_, Infallible>(Response::new(Full::new(Bytes::new()))) }
		});
		hyper::server::conn::http1::Builder::new()
			.serve_connection(TokioIo::new(LingeringClose::new(socket, unread)), service)
			.with_upgrades()
			.await
			.unwrap();
	});
	let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
	client
		.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nhost: example.com:443\r\n\r\n")
		.await
		.unwrap();
	let mut answer = Vec::new();
	client.read_to_end(&mut answer).await.unwrap();
	assert!(answer.starts_with(b"HTTP/1.1 200"));
	client.write_all(b"hello").await.unwrap();
	let input = tokio::time::timeout(Duration::from_secs(10), tunneled)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(&input, b"hello");
	gateway.abort();
}

#[tokio::test]
async fn an_upload_past_the_discard_limit_is_cut_off_and_counted() {
	let observed = Observed::start();
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let chunks = 4 * LINGER_BYTES as usize / CHUNK;
	let cut_off = tokio::time::timeout(
		Duration::from_secs(15),
		upload_then_read(&origin, "/v1/wrong-token/images/upload", chunks),
	)
	.await
	.unwrap();
	assert!(cut_off.is_err(), "{cut_off:?}");
	assert_eq!(
		observed.count(r#"aidash_egress_linger_limit_total{limit="bytes"}"#),
		1
	);
	shutdown(tasks, stop).await;
}

/// A refused client that keeps its connection open holds a slot until the time
/// limit and no longer.
#[tokio::test]
async fn lingering_connections_give_their_slots_back_at_the_time_limit() {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let address = origin.trim_start_matches("http://").to_owned();
	let started = Instant::now();
	let mut held = Vec::new();
	for _ in 0..CONNECTION_SLOTS {
		let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
		socket
			.write_all(
				b"POST /v1/wrong-token/images/upload HTTP/1.1\r\nhost: gateway\r\ncontent-length: 1048576\r\n\r\n",
			)
			.await
			.unwrap();
		let mut status = [0; 12];
		socket.read_exact(&mut status).await.unwrap();
		assert_eq!(&status, b"HTTP/1.1 403");
		held.push(socket);
	}
	tokio::time::timeout(Duration::from_secs(15), async {
		loop {
			let response = reqwest::get(format!("{origin}/v1/wrong-token/images/download/1"))
				.await
				.unwrap();
			if response.status() == StatusCode::FORBIDDEN {
				return;
			}
			assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
	})
	.await
	.unwrap();
	assert!(started.elapsed() >= LINGER_TIME);
	drop(held);
	shutdown(tasks, stop).await;
}

#[tokio::test(start_paused = true)]
async fn a_client_still_sending_at_the_time_limit_is_counted() {
	use tokio::io::AsyncWriteExt;
	let observed = Observed::start();
	let (socket, mut client) = tokio::io::duplex(1024);
	let closing = tokio::spawn(async move {
		let unread = UnreadBody(Arc::new(true.into()));
		LingeringClose::new(socket, unread).shutdown().await
	});
	client.write_all(b"unread").await.unwrap();
	tokio::time::sleep(LINGER_TIME + Duration::from_secs(1)).await;
	closing.await.unwrap().unwrap();
	assert_eq!(
		observed.count(r#"aidash_egress_linger_limit_total{limit="time"}"#),
		1
	);
}

#[tokio::test(start_paused = true)]
async fn a_request_without_a_body_closes_at_once() {
	use tokio::io::AsyncWriteExt;
	let (socket, _client) = tokio::io::duplex(1024);
	let started = tokio::time::Instant::now();
	LingeringClose::new(socket, UnreadBody::default())
		.shutdown()
		.await
		.unwrap();
	assert_eq!(started.elapsed(), Duration::ZERO);
}

/// What the gateway logs and counts on this test's thread. `#[tokio::test]` runs
/// every task on one thread, so spawned connections are observed too.
pub(super) struct Observed {
	logs: Arc<Mutex<Vec<u8>>>,
	metrics: metrics_exporter_prometheus::PrometheusHandle,
	_log: tracing::subscriber::DefaultGuard,
	_metrics: metrics::LocalRecorderGuard<'static>,
}
impl Observed {
	pub(super) fn start() -> Self {
		struct Writer(Arc<Mutex<Vec<u8>>>);
		impl std::io::Write for Writer {
			fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
				self.0.lock().unwrap().extend_from_slice(bytes);
				Ok(bytes.len())
			}
			fn flush(&mut self) -> std::io::Result<()> {
				Ok(())
			}
		}
		let logs = Arc::new(Mutex::new(Vec::new()));
		let writer = logs.clone();
		let subscriber = tracing_subscriber::fmt()
			.with_max_level(tracing::Level::TRACE)
			.with_writer(move || Writer(writer.clone()))
			.finish();
		let recorder = Box::leak(Box::new(
			metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder(),
		));
		let metrics = recorder.handle();
		Self {
			logs,
			metrics,
			_log: tracing::subscriber::set_default(subscriber),
			_metrics: metrics::set_default_local_recorder(recorder),
		}
	}

	pub(super) fn logs(&self) -> String {
		String::from_utf8_lossy(&self.logs.lock().unwrap()).into_owned()
	}

	pub(super) fn count(&self, series: &str) -> u64 {
		self.metrics
			.render()
			.lines()
			.find_map(|line| line.strip_prefix(series)?.trim().parse().ok())
			.unwrap_or(0)
	}
}

/// An admission that fails with a message repeating what the request carried.
struct UnavailableAuthority;
#[async_trait]
impl Authority for UnavailableAuthority {
	async fn admit(&self, token: &str, _: &str, _: &str, path: &str, _: &str) -> Result<Admission> {
		Err(aidash_application::Error::External(format!(
			"pool timed out while admitting {token} for {path}"
		)))
	}
}

async fn gateway_with(
	authority: Arc<dyn Authority>,
) -> (String, tokio::task::JoinHandle<()>, watch::Sender<bool>) {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(async move { serve(listener, authority, receiver).await.unwrap() });
	(origin, gateway, stop)
}

#[tokio::test]
async fn an_unexpected_admission_failure_is_logged_and_counted_without_the_request_contents() {
	let observed = Observed::start();
	let (origin, gateway, stop) = gateway_with(Arc::new(UnavailableAuthority)).await;
	let response = reqwest::get(format!(
		"{origin}/v1/operation-token/images/private-path?q=private-query"
	))
	.await
	.unwrap();
	assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
	assert_eq!(
		response.text().await.unwrap(),
		r#"{"error":"EGRESS_UNAVAILABLE"}"#,
		"the Sandbox sees only the code"
	);
	let logs = observed.logs();
	assert!(logs.contains("Egress admission failed"), "{logs}");
	assert!(logs.contains("pool timed out"), "{logs}");
	assert!(logs.contains("images"), "{logs}");
	assert!(logs.contains("GET"), "{logs}");
	for private in ["operation-token", "private-path", "private-query"] {
		assert!(!logs.contains(private), "{private} in {logs}");
	}
	assert_eq!(
		observed.count(r#"aidash_egress_refused_total{reason="EGRESS_UNAVAILABLE"}"#),
		1
	);
	shutdown(vec![gateway], stop).await;
}

#[tokio::test]
async fn an_expected_refusal_is_counted_by_reason_and_not_logged_as_an_error() {
	let observed = Observed::start();
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let response = reqwest::get(format!("{origin}/v1/wrong-token/images/download/1"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::FORBIDDEN);
	assert_eq!(
		observed.count(r#"aidash_egress_refused_total{reason="egress_forbidden"}"#),
		1
	);
	assert!(!observed.logs().contains("ERROR"), "{}", observed.logs());
	shutdown(tasks, stop).await;
}

struct InactiveAuthority;
#[async_trait]
impl Authority for InactiveAuthority {
	async fn admit(&self, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Admission> {
		Err(aidash_application::Error::Conflict(
			aidash_domain::egress::OPERATION_INACTIVE.into(),
		))
	}
}

#[tokio::test]
async fn an_inactive_operation_is_refused_and_counted_by_its_own_reason() {
	let observed = Observed::start();
	let (origin, gateway, stop) = gateway_with(Arc::new(InactiveAuthority)).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/private-path"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::FORBIDDEN);
	assert_eq!(
		observed.count(r#"aidash_egress_refused_total{reason="operation_inactive"}"#),
		1
	);
	shutdown(vec![gateway], stop).await;
}

#[tokio::test]
async fn an_unreachable_upstream_is_logged_with_its_cause_and_without_the_url() {
	let observed = Observed::start();
	// Nothing listens on a port that was just released.
	let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream = format!("http://{}/", unused.local_addr().unwrap());
	drop(unused);
	let authority = FixtureAuthority {
		upstream,
		origin: "http://gateway.invalid".into(),
		audit: Arc::new(Audit::default()),
	};
	let audit = authority.audit.clone();
	let (origin, gateway, stop) = gateway_with(Arc::new(authority)).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/private-path"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
	assert_eq!(
		*audit.finished_records.lock().unwrap(),
		vec![(502, Some("upstream_unavailable".into()))]
	);
	let logs = observed.logs();
	assert!(logs.contains("Egress upstream failed"), "{logs}");
	assert!(logs.to_lowercase().contains("connect"), "{logs}");
	for private in ["operation-token", "private-path", "canary-tenant-secret"] {
		assert!(!logs.contains(private), "{private} in {logs}");
	}
	shutdown(vec![gateway], stop).await;
}

#[tokio::test]
async fn full_connection_slots_answer_with_a_named_503_and_then_drop() {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let observed = Observed::start();
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let address = origin.trim_start_matches("http://").to_owned();
	let mut held = Vec::new();
	for _ in 0..CONNECTION_SLOTS {
		held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
	}

	let mut refused = tokio::net::TcpStream::connect(&address).await.unwrap();
	refused
		.write_all(b"GET /v1/operation-token/images/download/1 HTTP/1.1\r\nhost: gateway\r\n\r\n")
		.await
		.unwrap();
	let mut answer = Vec::new();
	tokio::time::timeout(Duration::from_secs(5), refused.read_to_end(&mut answer))
		.await
		.unwrap()
		.unwrap();
	let answer = String::from_utf8_lossy(&answer);
	assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
	assert!(answer.contains(CONNECTION_LIMIT), "{answer}");

	// Silent clients occupy every refusal, so the next connection is dropped.
	for _ in 0..REFUSAL_SLOTS {
		held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
	}
	let mut dropped = tokio::net::TcpStream::connect(&address).await.unwrap();
	let mut nothing = Vec::new();
	let _ = tokio::time::timeout(Duration::from_secs(1), dropped.read_to_end(&mut nothing))
		.await
		.expect("the connection is closed before the refusal window ends");
	assert!(nothing.is_empty());

	assert_eq!(
		observed.count(r#"aidash_egress_connections_refused_total{outcome="answered"}"#),
		1 + REFUSAL_SLOTS as u64
	);
	assert_eq!(
		observed.count(r#"aidash_egress_connections_refused_total{outcome="dropped"}"#),
		1
	);
	assert_eq!(
		observed
			.logs()
			.matches("Egress connection slots are full")
			.count(),
		1,
		"the log is thinned"
	);
	drop(held);
	shutdown(tasks, stop).await;
}

/// Fails the given number of calls of each kind, then succeeds.
#[derive(Default)]
struct FlakyMeter {
	audit: Audit,
	charge_failures: AtomicU64,
	finish_failures: AtomicU64,
}
impl FlakyMeter {
	fn failing(charges: u64, finishes: u64) -> Arc<Self> {
		Arc::new(Self {
			charge_failures: AtomicU64::new(charges),
			finish_failures: AtomicU64::new(finishes),
			..Default::default()
		})
	}
}
fn take_failure(left: &AtomicU64) -> Result<()> {
	match left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)) {
		Ok(_) => Err(aidash_application::Error::External("pool timed out".into())),
		Err(_) => Ok(()),
	}
}
#[async_trait]
impl Meter for FlakyMeter {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		take_failure(&self.charge_failures)?;
		self.audit.charge(direction, bytes).await
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		take_failure(&self.finish_failures)?;
		self.audit.finish(status, times, error).await
	}
}

fn completion(meter: Arc<dyn Meter>, expected: Option<u64>) -> Completion {
	Completion {
		debits: Arc::new(Debits::new(
			meter,
			Label {
				service: "images".into(),
				method: "GET".into(),
			},
		)),
		started: Instant::now(),
		deadline: tokio::time::Instant::now() + Duration::from_secs(600),
		status: 200,
		expected,
		finished: false,
		admission_ms: 7,
		settle_ms: 0,
		monitor: tokio::spawn(async {}),
	}
}

/// A store that takes `DEBIT` for each debit.
struct SlowDebit(Audit);
const DEBIT: Duration = Duration::from_secs(3);
#[async_trait]
impl Meter for SlowDebit {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		tokio::time::sleep(DEBIT).await;
		self.0.charge(direction, bytes).await
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		self.0.finish(status, times, error).await
	}
}

fn assert_final_debit_timed(audit: &Audit) {
	let times = audit.times.lock().unwrap().clone();
	assert_eq!(times.len(), 1);
	assert_eq!(times[0].admission_ms, 7);
	assert!(
		(millis(DEBIT)..millis(DEBIT) + 100).contains(&times[0].settle_ms),
		"{times:?}"
	);
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_ends_itself_records_how_long_its_final_debit_took() {
	let meter = Arc::new(SlowDebit(Audit::default()));
	let mut completion = completion(meter.clone(), None);
	completion.debits.count(Direction::Received, 100);
	assert!(completion.finish(None).await.is_none());
	assert_final_debit_timed(&meter.0);
}

#[tokio::test(start_paused = true)]
async fn a_response_that_stops_at_its_length_records_how_long_its_final_debit_took() {
	let meter = Arc::new(SlowDebit(Audit::default()));
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET).await;
	assert_final_debit_timed(&meter.0);
}

#[tokio::test(start_paused = true)]
async fn a_complete_download_settles_after_a_storage_stall() {
	let observed = Observed::start();
	let meter = FlakyMeter::failing(3, 3);
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET).await;
	assert_eq!(meter.audit.received.load(Ordering::Relaxed), 100);
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, None)]
	);
	assert_eq!(
		observed.count(r#"aidash_egress_settle_failures_total{stage="debit",outcome="retried"}"#),
		3
	);
	assert!(!observed.logs().contains("ERROR"), "{}", observed.logs());
	// The retries are part of settling: waits of 1, 2 and 4 seconds.
	let times = meter.audit.times.lock().unwrap().clone();
	assert!(times[0].settle_ms >= 7_000, "{times:?}");
}

#[tokio::test(start_paused = true)]
async fn a_shorter_body_than_its_content_length_is_recorded_as_interrupted() {
	let meter = FlakyMeter::failing(0, 0);
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 99);
	drop(completion);
	tokio::time::sleep(Duration::from_secs(1)).await;
	assert_eq!(meter.audit.received.load(Ordering::Relaxed), 99);
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, Some("stream_interrupted".into()))]
	);
}

#[tokio::test(start_paused = true)]
async fn settling_gives_up_with_an_error_and_leaves_the_record_unsettled() {
	let observed = Observed::start();
	let meter = FlakyMeter::failing(u64::MAX, u64::MAX);
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET * 2).await;
	assert!(meter.audit.finished_records.lock().unwrap().is_empty());
	for (stage, message) in [
		("debit", "Egress debit abandoned"),
		("record", "Egress request record abandoned"),
	] {
		assert_eq!(
			observed.count(&format!(
				r#"aidash_egress_settle_failures_total{{stage="{stage}",outcome="abandoned"}}"#
			)),
			1
		);
		assert!(observed.logs().contains(message), "{}", observed.logs());
	}
	assert!(
		observed.logs().contains("pool timed out"),
		"{}",
		observed.logs()
	);
}

#[tokio::test(start_paused = true)]
async fn a_temporary_debit_failure_stops_the_stream_but_keeps_its_bytes() {
	let observed = Observed::start();
	let meter = FlakyMeter::failing(1, 0);
	let debits = Debits::new(
		meter.clone(),
		Label {
			service: "images".into(),
			method: "GET".into(),
		},
	);
	debits.count(Direction::Received, 100);
	let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
	assert_eq!(
		debits
			.flush(Direction::Received, deadline)
			.await
			.unwrap_err()
			.code,
		aidash_domain::egress::UNAVAILABLE
	);
	assert!(
		observed.logs().contains("pool timed out"),
		"{}",
		observed.logs()
	);
	drop(debits);
	tokio::time::sleep(Duration::from_secs(1)).await;
	assert_eq!(meter.audit.received.load(Ordering::Relaxed), 100);
}

#[tokio::test(start_paused = true)]
async fn a_failed_final_record_retries_without_holding_the_response() {
	let meter = FlakyMeter::failing(0, 2);
	let mut completion = completion(meter.clone(), None);
	completion.close(None).await;
	assert!(
		meter.audit.finished_records.lock().unwrap().is_empty(),
		"the first attempt failed and the rest run in the background"
	);
	tokio::time::sleep(Duration::from_secs(5)).await;
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, None)]
	);
}

#[tokio::test(start_paused = true)]
async fn a_refused_final_debit_is_recorded_and_not_retried() {
	struct Refusing(Audit);
	#[async_trait]
	impl Meter for Refusing {
		async fn charge(&self, _: Direction, _: usize) -> Result<()> {
			self.0.charges.fetch_add(1, Ordering::Relaxed);
			Err(aidash_application::Error::Conflict(
				aidash_domain::egress::LIMIT_EXCEEDED.into(),
			))
		}
		async fn finish(
			&self,
			status: u16,
			times: RequestTimes,
			error: Option<&str>,
		) -> Result<()> {
			self.0.finish(status, times, error).await
		}
	}
	let meter = Arc::new(Refusing(Audit::default()));
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET).await;
	assert_eq!(meter.0.charges.load(Ordering::Relaxed), 1);
	assert_eq!(
		*meter.0.finished_records.lock().unwrap(),
		vec![(200, Some("egress_limit_exceeded".into()))]
	);
}

/// Keeps its record from being written for `STALL`, then writes it.
struct StalledRecord(Audit);
const STALL: Duration = Duration::from_secs(3);
#[async_trait]
impl Meter for StalledRecord {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()> {
		self.0.charge(direction, bytes).await
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		tokio::time::sleep(STALL).await;
		self.0.finish(status, times, error).await
	}
}

#[tokio::test(start_paused = true)]
async fn a_stalled_record_does_not_hold_the_response_for_longer_than_a_moment() {
	let meter = Arc::new(StalledRecord(Audit::default()));
	let mut completion = completion(meter.clone(), None);
	let began = tokio::time::Instant::now();
	completion.close(None).await;
	assert!(began.elapsed() <= RECORD_WAIT + Duration::from_millis(100));
	assert!(meter.0.finished_records.lock().unwrap().is_empty());
	tokio::time::sleep(STALL).await;
	assert_eq!(*meter.0.finished_records.lock().unwrap(), vec![(200, None)]);
}

/// Hyper drops a response body when its client disconnects, wherever the body
/// is waiting.
#[tokio::test(start_paused = true)]
async fn a_record_stalled_in_the_store_still_arrives_after_the_client_disconnects() {
	let meter = Arc::new(StalledRecord(Audit::default()));
	let mut completion = completion(meter.clone(), None);
	let disconnected = tokio::time::timeout(RECORD_WAIT / 2, completion.close(None)).await;
	assert!(disconnected.is_err(), "the client left mid-write");
	drop(completion);
	tokio::time::sleep(STALL * 2).await;
	assert_eq!(
		*meter.0.finished_records.lock().unwrap(),
		vec![(200, None)],
		"the write is not tied to the response"
	);
}

/// The client leaves once it has the body, while the gateway is still writing the
/// record that ends the response. Hyper drops the response stream, wherever it
/// is waiting, when its connection closes.
#[tokio::test]
async fn a_client_that_leaves_while_its_record_is_written_does_not_lose_the_record() {
	let meter = Arc::new(StalledRecord(Audit::default()));
	let (origin, tasks, stop) = gateway_metered_by(meter.clone()).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/download/1"))
		.await
		.unwrap();
	let mut body = response.bytes_stream();
	let mut total = 0;
	while total < CHUNK {
		total += body.next().await.unwrap().unwrap().len();
	}
	// The end of the body waits for the record, which the store is stalling.
	tokio::time::sleep(Duration::from_millis(200)).await;
	drop(body);
	tokio::time::timeout(2 * STALL, meter.0.finished.notified())
		.await
		.expect("the record is written although its response is gone");
	assert_eq!(*meter.0.finished_records.lock().unwrap(), vec![(200, None)]);
	shutdown(tasks, stop).await;
}

#[tokio::test(start_paused = true)]
async fn a_final_debit_that_fails_for_good_is_recorded_as_abandoned() {
	let observed = Observed::start();
	let meter = FlakyMeter::failing(u64::MAX, 0);
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET * 2).await;
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, Some(DEBIT_ABANDONED.into()))],
		"a complete download whose last charge was lost is not a plain success"
	);
	assert_eq!(
		observed.count(r#"aidash_egress_settle_failures_total{stage="debit",outcome="abandoned"}"#),
		1
	);
	// What the time counts is the trying.
	let times = meter.audit.times.lock().unwrap().clone();
	assert!(times[0].settle_ms >= millis(SETTLE_BUDGET), "{times:?}");
}

/// Refuses uploads and cannot be asked about downloads.
struct RefusedUpload(Audit);
#[async_trait]
impl Meter for RefusedUpload {
	async fn charge(&self, direction: Direction, _: usize) -> Result<()> {
		match direction {
			Direction::Sent => Err(aidash_application::Error::Conflict(
				aidash_domain::egress::LIMIT_EXCEEDED.into(),
			)),
			Direction::Received => {
				Err(aidash_application::Error::External("pool timed out".into()))
			}
		}
	}
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()> {
		self.0.finish(status, times, error).await
	}
}

#[tokio::test(start_paused = true)]
async fn a_refusal_wins_over_a_debit_that_was_given_up() {
	let meter = Arc::new(RefusedUpload(Audit::default()));
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Sent, 10);
	completion.debits.count(Direction::Received, 100);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET * 2).await;
	assert_eq!(
		*meter.0.finished_records.lock().unwrap(),
		vec![(200, Some("egress_limit_exceeded".into()))]
	);
}

#[tokio::test(start_paused = true)]
async fn an_incomplete_download_stays_interrupted_whatever_its_debit_did() {
	let meter = FlakyMeter::failing(u64::MAX, 0);
	let completion = completion(meter.clone(), Some(100));
	completion.debits.count(Direction::Received, 99);
	drop(completion);
	tokio::time::sleep(SETTLE_BUDGET * 2).await;
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, Some("stream_interrupted".into()))]
	);
}

#[tokio::test(start_paused = true)]
async fn a_final_debit_that_fails_for_a_moment_does_not_cut_a_finished_response() {
	let observed = Observed::start();
	let meter = FlakyMeter::failing(2, 0);
	let mut completion = completion(meter.clone(), None);
	completion.debits.count(Direction::Received, 100);
	assert!(
		completion.finish(None).await.is_none(),
		"the body is complete; only a decision stops it"
	);
	drop(completion);
	tokio::time::sleep(Duration::from_secs(5)).await;
	assert_eq!(meter.audit.received.load(Ordering::Relaxed), 100);
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, None)]
	);
	// The failed attempt and the wait before the retry are part of settling.
	let times = meter.audit.times.lock().unwrap().clone();
	assert!(times[0].settle_ms >= 1_000, "{times:?}");
	assert_eq!(
		observed.count(r#"aidash_egress_settle_failures_total{stage="debit",outcome="retried"}"#),
		2
	);
}

#[tokio::test]
async fn a_download_survives_a_final_debit_that_the_store_drops() {
	// Fewer bytes than a batch, so the only debit is the last one.
	let meter = FlakyMeter::failing(1, 0);
	let (origin, tasks, stop) = gateway_metered_by(meter.clone()).await;
	let response = reqwest::get(format!("{origin}/v1/operation-token/images/download/4"))
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let (total, clean) = drain(response).await;
	assert_eq!(total, 4 * CHUNK);
	assert!(clean, "the client gets the whole body and a clean end");
	tokio::time::timeout(Duration::from_secs(10), meter.audit.finished.notified())
		.await
		.unwrap();
	assert_eq!(meter.audit.received.load(Ordering::Relaxed), total as u64);
	assert_eq!(
		*meter.audit.finished_records.lock().unwrap(),
		vec![(200, None)]
	);
	shutdown(tasks, stop).await;
}

#[test]
fn only_what_could_hold_a_credential_is_taken_out_of_an_error() {
	let cause = || "pool timed out admitting a path for a token".to_owned();
	for short in ["", "a", "pa", "pat"] {
		assert_eq!(without(cause(), short, "[x]"), cause(), "{short:?}");
	}
	assert_eq!(
		without(cause(), "path", "[x]"),
		"pool timed out admitting a [x] for a token"
	);
}

#[tokio::test]
async fn a_short_path_does_not_garble_the_logged_cause() {
	let observed = Observed::start();
	let (origin, gateway, stop) = gateway_with(Arc::new(UnavailableAuthority)).await;
	// An empty path, and one a sandbox picked to match as many letters as it can.
	for path in ["images", "images/a"] {
		let response = reqwest::get(format!("{origin}/v1/operation-token/{path}"))
			.await
			.unwrap();
		assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
	}
	let logs = observed.logs();
	assert_eq!(
		logs.matches("pool timed out while admitting [token] for")
			.count(),
		2,
		"{logs}"
	);
	assert!(logs.contains("admitting [token] for a"), "{logs}");
	assert!(!logs.contains("operation-token"), "{logs}");
	shutdown(vec![gateway], stop).await;
}

/// A client that needs longer than the window for answering to send its upload.
#[tokio::test]
async fn a_slow_upload_still_reads_the_refusal_for_full_slots() {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let observed = Observed::start();
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let address = origin.trim_start_matches("http://").to_owned();
	let mut held = Vec::new();
	for _ in 0..CONNECTION_SLOTS {
		held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
	}
	let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
	socket
		.write_all(
			b"POST /v1/operation-token/images/upload HTTP/1.1\r\nhost: gateway\r\ncontent-length: 1048576\r\n\r\n",
		)
		.await
		.unwrap();
	let started = Instant::now();
	for _ in 0..6 {
		tokio::time::sleep(REFUSAL_WINDOW / 4).await;
		socket.write_all(&[1; CHUNK]).await.unwrap();
	}
	assert!(started.elapsed() > REFUSAL_WINDOW);
	let mut answer = Vec::new();
	tokio::time::timeout(Duration::from_secs(15), socket.read_to_end(&mut answer))
		.await
		.unwrap()
		.unwrap();
	let answer = String::from_utf8_lossy(&answer);
	assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
	assert!(answer.contains(CONNECTION_LIMIT), "{answer}");
	// The gateway closes its side with the answer and then discards what is
	// left. The client is still open when that runs out of time, and that is counted.
	tokio::time::sleep((LINGER_TIME + Duration::from_secs(1)).saturating_sub(started.elapsed()))
		.await;
	assert_eq!(
		observed.count(r#"aidash_egress_linger_limit_total{limit="time"}"#),
		1
	);
	// The refusal slot is back.
	let mut next = tokio::net::TcpStream::connect(&address).await.unwrap();
	next.write_all(b"GET /v1/operation-token/images/download/1 HTTP/1.1\r\nhost: gateway\r\n\r\n")
		.await
		.unwrap();
	let mut answer = Vec::new();
	tokio::time::timeout(Duration::from_secs(5), next.read_to_end(&mut answer))
		.await
		.unwrap()
		.unwrap();
	assert!(String::from_utf8_lossy(&answer).contains(CONNECTION_LIMIT));
	assert_eq!(
		observed.count(r#"aidash_egress_connections_refused_total{outcome="answered"}"#),
		2
	);
	assert_eq!(
		observed.count(r#"aidash_egress_connections_refused_total{outcome="dropped"}"#),
		0
	);
	drop(held);
	shutdown(tasks, stop).await;
}

/// Every refusal slot is held by a client that has its answer and keeps its
/// connection open: the next one is dropped, and all of them are back once the
/// discard has run out of time (#726).
#[tokio::test]
async fn refusal_slots_are_held_for_the_discard_and_then_given_back() {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let observed = Observed::start();
	let (origin, _audit, tasks, stop) = metered_gateway(u64::MAX, false).await;
	let address = origin.trim_start_matches("http://").to_owned();
	let mut held = Vec::new();
	for _ in 0..CONNECTION_SLOTS {
		held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
	}
	let started = Instant::now();
	for _ in 0..REFUSAL_SLOTS {
		let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
		socket
			.write_all(
				b"POST /v1/operation-token/images/upload HTTP/1.1\r\nhost: gateway\r\ncontent-length: 1048576\r\n\r\n",
			)
			.await
			.unwrap();
		socket.write_all(&[1; 1024]).await.unwrap();
		let mut status = [0; 12];
		socket.read_exact(&mut status).await.unwrap();
		assert_eq!(&status, b"HTTP/1.1 503");
		// Bytes that arrive after the answer are the ones the gateway discards.
		socket.write_all(&[1; 1024]).await.unwrap();
		held.push(socket);
	}
	// Every refusal is lingering, so there is no slot to answer one more.
	let mut dropped = tokio::net::TcpStream::connect(&address).await.unwrap();
	let mut nothing = Vec::new();
	tokio::time::timeout(Duration::from_secs(1), dropped.read_to_end(&mut nothing))
		.await
		.expect("the connection is closed unanswered")
		.unwrap();
	assert!(nothing.is_empty());
	assert_eq!(
		observed.count(r#"aidash_egress_connections_refused_total{outcome="dropped"}"#),
		1
	);
	// The discard ends at its time limit whether or not the clients leave.
	tokio::time::sleep((LINGER_TIME + Duration::from_secs(1)).saturating_sub(started.elapsed()))
		.await;
	assert_eq!(
		observed.count(r#"aidash_egress_linger_limit_total{limit="time"}"#),
		REFUSAL_SLOTS as u64
	);
	let mut next = tokio::net::TcpStream::connect(&address).await.unwrap();
	next.write_all(b"GET /v1/operation-token/images/download/1 HTTP/1.1\r\nhost: gateway\r\n\r\n")
		.await
		.unwrap();
	let mut answer = Vec::new();
	tokio::time::timeout(Duration::from_secs(5), next.read_to_end(&mut answer))
		.await
		.unwrap()
		.unwrap();
	assert!(String::from_utf8_lossy(&answer).contains(CONNECTION_LIMIT));
	drop(held);
	shutdown(tasks, stop).await;
}
