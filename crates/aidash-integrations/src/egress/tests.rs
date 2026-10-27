use super::*;
use aidash_application::Result;
use aidash_domain::{egress::ExternalService, keyless::Destination};
use async_trait::async_trait;
use axum::{Router, body::Body as AxumBody, routing::any};
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
	async fn finish(&self, _: u16, _: u64, _: Option<&str>) -> Result<()> {
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
