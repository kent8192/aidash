//! Opt-in provider -> native worker -> installed Enterprise Meter acceptance.
//! No receipt, provider timestamp, operation intent or billing row is seeded.
use super::*;
use aidash_server::capabilities::Runtime;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use reinhardt::query::{Alias, Expr, ExprTrait as _, Order, Query, SelectStatement};
use std::{
	collections::{BTreeMap, BTreeSet},
	fs::{File, OpenOptions},
	io::Write,
	net::SocketAddr,
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
	process::Stdio,
	sync::Mutex,
	time::Duration as Wait,
};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
};

type Environment = Shared<BoxFuture<'static, Arc<common::TestEnvironment>>>;

#[rstest::fixture]
fn isolated_runtime(
	#[from(common::isolated_test_environment)] environment: Environment,
	#[from(reinhardt::test::fixtures::http_client)] client: reqwest::Client,
) -> common::RuntimeFuture {
	common::runtime(
		common::execution_database(environment, "execution_real_allocation", 12),
		client,
	)
}

/// Only explicit call sites below can append evidence. Secrets and ordinary
/// operation request/output payloads never enter this file or its in-memory copy.
#[derive(Clone)]
struct Evidence(Arc<Mutex<(File, Vec<Value>)>>);

impl Evidence {
	fn new(directory: &Path) -> Self {
		let file = OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(directory.join("native.jsonl"))
			.unwrap();
		File::open(directory).unwrap().sync_all().unwrap();
		Self(Arc::new(Mutex::new((file, vec![]))))
	}
	fn record(&self, value: Value) -> usize {
		let mut saved = self.0.lock().unwrap();
		serde_json::to_writer(&mut saved.0, &value).unwrap();
		saved.0.write_all(b"\n").unwrap();
		saved.0.sync_all().unwrap();
		saved.1.push(value);
		saved.1.len() - 1
	}
	fn events(&self) -> Vec<Value> {
		self.0.lock().unwrap().1.clone()
	}
	fn receipt(&self, allocation: &str, after: usize) -> Option<Value> {
		self.events()
			.into_iter()
			.skip(after)
			.filter(|event| event["kind"] == "allocation_page")
			.flat_map(|event| {
				event["response"]["receipts"]
					.as_array()
					.cloned()
					.unwrap_or_default()
			})
			.find(|receipt| receipt["allocation_id"] == allocation)
	}
	fn ack(&self, allocation: &str, succeeded: bool) -> Option<Value> {
		self.events().into_iter().find(|event| {
			event["kind"] == "allocation_ack"
				&& event["allocation"] == allocation
				&& if succeeded {
					event["status"] == 200 && event["response"]["acknowledged"] == true
				} else {
					event["transport_error"] == "upstream_io"
				}
		})
	}
	fn operation_ack(&self, operation: Uuid) -> bool {
		self.events().iter().any(|event| {
			event["kind"] == "operation_ack"
				&& event["operation"] == operation.to_string()
				&& event["status"] == 200
				&& event["response"]["acknowledged"] == true
		})
	}
}

fn only(value: &Value, keys: &[&str]) -> Value {
	Value::Object(
		keys.iter()
			.filter_map(|key| value.get(*key).map(|v| ((*key).into(), v.clone())))
			.collect(),
	)
}

/// HTTP/1.1 pass-through for the controller's bounded Content-Length protocol.
/// Forward original bytes, including private authentication, unchanged. A real
/// unavailable upstream closes the socket; it never becomes a synthetic reply.
struct Recorder {
	endpoint: String,
	handle: tokio::task::JoinHandle<()>,
}

impl Recorder {
	async fn start(upstream: SocketAddr, evidence: Evidence) -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let handle = tokio::spawn(async move {
			let mut requests = tokio::task::JoinSet::new();
			loop {
				tokio::select! {
					accepted = listener.accept() => {
						let (stream, _) = accepted.unwrap();
						requests.spawn(forward(stream, upstream, evidence.clone()));
					}
					Some(result) = requests.join_next(), if !requests.is_empty() => {
						result.expect("HTTP observer must retain evidence before forwarding");
					}
				}
			}
		});
		Self { endpoint, handle }
	}
}
impl Drop for Recorder {
	fn drop(&mut self) {
		self.handle.abort();
	}
}

async fn message(stream: &mut TcpStream) -> std::io::Result<(Vec<u8>, usize)> {
	let mut bytes = Vec::new();
	let end = loop {
		if bytes.ends_with(b"\r\n\r\n") {
			break bytes.len();
		}
		if bytes.len() >= 65536 {
			return Err(std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				"HTTP observer header bound",
			));
		}
		bytes.push(stream.read_u8().await?);
	};
	let headers = std::str::from_utf8(&bytes).map_err(|_| {
		std::io::Error::new(std::io::ErrorKind::InvalidData, "HTTP header encoding")
	})?;
	let mut length = 0;
	for line in headers.lines().skip(1) {
		if let Some((name, value)) = line.split_once(':') {
			if name.eq_ignore_ascii_case("transfer-encoding") {
				return Err(std::io::Error::new(
					std::io::ErrorKind::InvalidData,
					"unsupported controller transfer framing",
				));
			}
			if name.eq_ignore_ascii_case("content-length") {
				length = value.trim().parse::<usize>().map_err(|_| {
					std::io::Error::new(std::io::ErrorKind::InvalidData, "HTTP length")
				})?;
			}
		}
	}
	if length > 16 * 1024 * 1024 {
		return Err(std::io::Error::new(
			std::io::ErrorKind::InvalidData,
			"HTTP observer body bound",
		));
	}
	bytes.resize(end + length, 0);
	stream.read_exact(&mut bytes[end..]).await?;
	Ok((bytes, end))
}

async fn forward(mut incoming: TcpStream, upstream: SocketAddr, evidence: Evidence) {
	let (request, body_offset) =
		match tokio::time::timeout(Wait::from_secs(180), message(&mut incoming)).await {
			Ok(Ok(message)) => message,
			Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => return,
			_ => panic!("HTTP observer could not read the actual request"),
		};
	let line = std::str::from_utf8(&request[..body_offset])
		.unwrap()
		.lines()
		.next()
		.unwrap();
	let path = line.split_whitespace().nth(1).unwrap();
	let route = path.split('?').next().unwrap();
	let parts: Vec<_> = route.split('/').collect();
	let mut event = if route == "/v1/health" {
		Some(json!({"kind":"health"}))
	} else if route == "/v1/allocations/receipts" {
		Some(json!({"kind":"allocation_page", "path":path}))
	} else if parts.len() == 5 && parts[1] == "v1" && parts[4] == "ack" {
		let body: Value = serde_json::from_slice(&request[body_offset..]).unwrap();
		match parts[2] {
			"allocations" => Some(json!({"kind":"allocation_ack", "allocation":parts[3],
				"request":body,"request_bytes":std::str::from_utf8(&request[body_offset..]).unwrap()})),
			"operations" => Some(json!({"kind":"operation_ack", "operation":parts[3],
				"request":only(&body, &["digest","output_digest"])})),
			_ => None,
		}
	} else {
		None
	};
	let response = tokio::time::timeout(Wait::from_secs(180), async {
		let mut outgoing = TcpStream::connect(upstream).await?;
		outgoing.write_all(&request).await?;
		message(&mut outgoing).await
	})
	.await;
	let (reply, offset) = match response {
		Ok(Ok(message)) => message,
		Ok(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => {
			panic!("HTTP observer could not preserve the actual response framing")
		}
		_ => {
			if let Some(ref mut event) = event {
				event["transport_error"] = json!("upstream_io");
				evidence.record(event.clone());
			}
			return;
		}
	};
	if let Some(ref mut event) = event {
		let status: u16 = std::str::from_utf8(&reply[..offset])
			.unwrap()
			.split_whitespace()
			.nth(1)
			.unwrap()
			.parse()
			.unwrap();
		let body: Value =
			serde_json::from_slice(&reply[offset..]).expect("observed controller JSON");
		event["status"] = json!(status);
		if event["kind"] == "operation_ack" {
			event["response"] = only(
				&body,
				&[
					"operation_id",
					"digest",
					"status",
					"termination_confirmed",
					"acknowledged",
					"acknowledged_output_digest",
				],
			);
		} else {
			event["response"] = body;
			event["response_bytes"] = json!(std::str::from_utf8(&reply[offset..]).unwrap());
		}
		evidence.record(event.clone());
	}
	// The peer may have timed out; that does not invalidate the captured actual
	// response. No observer changes a real status, header, authentication or body.
	let _ = incoming.write_all(&reply).await;
}

struct Worker {
	stop: tokio::sync::watch::Sender<bool>,
	handle: tokio::task::JoinHandle<aidash_server::Result<()>>,
}
impl Worker {
	fn start(d: &Cloud) -> Self {
		let (stop, receiver) = tokio::sync::watch::channel(false);
		Self {
			stop,
			handle: tokio::spawn(aidash_server::capabilities::operations::run(
				d.federation.store.clone(),
				receiver,
			)),
		}
	}
	fn live(&self, recorder: &Recorder) {
		assert!(
			!self.handle.is_finished(),
			"normal capability worker exited before allocation drain"
		);
		assert!(
			!recorder.handle.is_finished(),
			"HTTP evidence observer exited"
		);
	}
	async fn finish(mut self) {
		self.stop.send(true).unwrap();
		tokio::time::timeout(Wait::from_secs(30), &mut self.handle)
			.await
			.unwrap()
			.unwrap()
			.unwrap();
	}
}
impl Drop for Worker {
	fn drop(&mut self) {
		let _ = self.stop.send(true);
		self.handle.abort();
	}
}

/// The shell gate retains the original child unreaped until Cargo exits. This
/// exact PID therefore cannot be recycled while the test sends its normal TERM.
struct Controller {
	owner: Value,
	directory: PathBuf,
	endpoint: String,
	token: String,
	client: reqwest::Client,
	restarted: Option<tokio::process::Child>,
}
impl Controller {
	fn owned(profile: &aidash_server::capabilities::Profile) -> Self {
		let owner_path = PathBuf::from(
			std::env::var("AIDASH_ALLOCATION_GATE_OWNER")
				.expect("use the Enterprise runtime selector"),
		);
		let owner: Value = serde_json::from_slice(&std::fs::read(&owner_path).unwrap()).unwrap();
		let profile_path =
			std::fs::canonicalize(std::env::var("AIDASH_CAPABILITY_PROFILE").unwrap()).unwrap();
		assert_eq!(Path::new(owner["profile"].as_str().unwrap()), profile_path);
		let directory = profile_path.parent().unwrap().to_owned();
		assert_eq!(owner_path, directory.join("allocation-gate.json"));
		let runner = profile.runner.as_ref().unwrap();
		assert_eq!(runner.journal.as_ref().unwrap(), &directory.join("journal"));
		assert_eq!(
			profile.idle_seconds, 60,
			"both controller and Store must receive setup's bounded idle profile"
		);
		Self {
			owner,
			directory,
			endpoint: runner.endpoint.clone(),
			token: std::env::var(&runner.token_env).unwrap(),
			client: reqwest::Client::builder()
				.no_proxy()
				.timeout(Wait::from_secs(10))
				.build()
				.unwrap(),
			restarted: None,
		}
	}
	async fn get(&self, path: &str) -> reqwest::Result<Value> {
		self.client
			.get(format!("{}{path}", self.endpoint))
			.bearer_auth(&self.token)
			.send()
			.await?
			.error_for_status()?
			.json()
			.await
	}
	async fn stop_initial(&self) {
		let pid = self.owner["pid"].as_u64().unwrap();
		assert!(pid > 1);
		let status = tokio::process::Command::new("kill")
			.args(["-TERM", &pid.to_string()])
			.status()
			.await
			.unwrap();
		assert!(
			status.success(),
			"stop only the controller retained by the gate"
		);
		tokio::time::timeout(Wait::from_secs(20), async {
			while self.get("/v1/health").await.is_ok() {
				tokio::time::sleep(Wait::from_millis(100)).await;
			}
		})
		.await
		.expect("owned controller becomes unavailable");
	}
	async fn restart(&mut self, image: &str, instance: &Value, evidence: &Evidence) {
		let log = OpenOptions::new()
			.create_new(true)
			.write(true)
			.mode(0o600)
			.open(self.directory.join("allocation-controller-restart.log"))
			.unwrap();
		let source = Path::new(self.owner["source"].as_str().unwrap());
		self.restarted = Some(
			tokio::process::Command::new(self.owner["python"].as_str().unwrap())
				.arg(source.join("scripts/start-capability-runner.py"))
				.arg(&self.directory)
				.stdin(Stdio::null())
				.stdout(log.try_clone().unwrap())
				.stderr(log)
				.kill_on_drop(true)
				.spawn()
				.unwrap(),
		);
		tokio::time::timeout(Wait::from_secs(180), async {
			loop {
				assert!(
					self.restarted
						.as_mut()
						.unwrap()
						.try_wait()
						.unwrap()
						.is_none(),
					"owned restarted controller exited"
				);
				if let Ok(health) = self.get("/v1/health").await {
					assert_eq!(health["verified"], true);
					assert_eq!(health["python_verified"], true);
					assert_eq!(health["image"], image);
					assert_eq!(&health["instance"], instance);
					evidence.record(json!({"kind":"restart_admission", "health":health}));
					break;
				}
				tokio::time::sleep(Wait::from_millis(200)).await;
			}
		})
		.await
		.expect("same-journal normal restart passes physical admission");
	}
	async fn finish(&mut self) {
		if let Some(child) = self.restarted.as_mut() {
			let pid = child.id().unwrap();
			assert!(
				tokio::process::Command::new("kill")
					.args(["-TERM", &pid.to_string()])
					.status()
					.await
					.unwrap()
					.success()
			);
			tokio::time::timeout(Wait::from_secs(20), child.wait())
				.await
				.unwrap()
				.unwrap();
		}
	}
	async fn remaining_pods(
		&self,
		runner: &aidash_server::capabilities::RunnerProfile,
		uids: &BTreeSet<String>,
	) -> Vec<Value> {
		let output = tokio::process::Command::new(runner.kubectl.as_ref().unwrap())
			.arg("--kubeconfig")
			.arg(runner.kubeconfig.as_ref().unwrap())
			.args([
				"--namespace",
				&runner.namespace,
				"get",
				"pods",
				"-o",
				"json",
			])
			.output()
			.await
			.unwrap();
		assert!(
			output.status.success(),
			"owned provider cleanup query failed"
		);
		let pods: Value = serde_json::from_slice(&output.stdout).unwrap();
		pods["items"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|pod| {
				pod["metadata"]["uid"]
					.as_str()
					.is_some_and(|uid| uids.contains(uid))
			})
			.map(|pod| only(&pod["metadata"], &["uid", "name", "resourceVersion"]))
			.collect()
	}
	async fn running_uid(
		&self,
		runner: &aidash_server::capabilities::RunnerProfile,
		name: &str,
	) -> Option<String> {
		let output = tokio::process::Command::new(runner.kubectl.as_ref().unwrap())
			.arg("--kubeconfig")
			.arg(runner.kubeconfig.as_ref().unwrap())
			.args([
				"--namespace",
				&runner.namespace,
				"get",
				"pod",
				name,
				"--ignore-not-found",
				"-o",
				"json",
			])
			.output()
			.await
			.unwrap();
		assert!(
			output.status.success(),
			"owned provider running query failed"
		);
		if output.stdout.is_empty() {
			return None;
		}
		let pod: Value = serde_json::from_slice(&output.stdout).unwrap();
		pod["status"]["containerStatuses"]
			.as_array()?
			.iter()
			.any(|state| state["name"] == "execution" && state["state"].get("running").is_some())
			.then(|| pod["metadata"]["uid"].as_str().unwrap().to_owned())
	}
}

async fn eventually<T>(
	worker: &Worker,
	recorder: &Recorder,
	description: &str,
	mut check: impl AsyncFnMut() -> Option<T>,
) -> T {
	tokio::time::timeout(Wait::from_secs(150), async {
		loop {
			worker.live(recorder);
			if let Some(value) = check().await {
				return value;
			}
			tokio::time::sleep(Wait::from_millis(200)).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("timed out: {description}"))
}

fn select(table: &str, columns: &[&str]) -> SelectStatement {
	Query::select()
		.columns(columns.iter().map(|column| Alias::new(*column)))
		.from(Alias::new(table))
		.to_owned()
}

async fn rows(d: &Cloud, scope: &TenancyScope, query: SelectStatement) -> Vec<Value> {
	d.control()
		.atomic(scope, async |tx| store::fetch::<Value>(tx, &query).await)
		.await
		.unwrap()
}

async fn facts(d: &Cloud, uid: &str) -> Value {
	let allocation: Uuid = uid.parse().unwrap();
	let raw = rows(
		d,
		&TenancyScope::NoTenant,
		select(
			"billing_pod_usage",
			&[
				"allocation",
				"receipt_digest",
				"tenant",
				"started_at",
				"ended_at",
				"started_ns",
				"ended_ns",
				"billable_ns",
				"millis",
			],
		)
		.and_where(Expr::col("allocation").eq(Expr::value(allocation)))
		.to_owned(),
	)
	.await;
	let segments = rows(
		d,
		&TenancyScope::NoTenant,
		select(
			"billing_usage_segments",
			&[
				"id",
				"allocation",
				"tenant",
				"starts_ns",
				"ends_ns",
				"status",
				"coverage_id",
				"account_id",
			],
		)
		.and_where(Expr::col("allocation").eq(Expr::value(allocation)))
		.order_by(Alias::new("starts_ns"), Order::Asc)
		.to_owned(),
	)
	.await;
	json!({"raw":raw,"segments":segments})
}

async fn operation(d: &Cloud, tenant: &str, id: Uuid) -> Value {
	rows(
		d,
		&TenancyScope::tenant(tenant).unwrap(),
		select("core_operations", &["state", "result"])
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_owned(),
	)
	.await
	.pop()
	.unwrap()
}

async fn idle_reset(d: &Cloud, tenant: &str, session: &Value) -> Option<Value> {
	rows(
		d,
		&TenancyScope::tenant(tenant).unwrap(),
		select("core_records", &["state", "data"])
			.and_where(Expr::col("kind").eq(Expr::value("python_session")))
			.to_owned(),
	)
	.await
	.into_iter()
	.find(|row| {
		row["data"]["session_id"] == *session
			&& row["state"] == "reset"
			&& row["data"]["reset_reason"] == "idle_timeout"
	})
	.map(|row| only(&row["data"], &["session_id", "last_used", "reset_reason"]))
}

fn provider(directory: &Path, names: &[String]) -> Vec<Value> {
	let mut found = vec![];
	for path in std::fs::read_dir(directory).unwrap() {
		let path = path.unwrap().path();
		if path.extension().and_then(|value| value.to_str()) != Some("json") {
			continue;
		}
		let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
		assert_ne!(value["observer_failed"], true, "provider capture failed");
		if names.iter().any(|name| value["metadata"]["name"] == *name) {
			found.push(value);
		}
	}
	found
}

fn pod_uids(snapshots: &[Value]) -> BTreeSet<String> {
	snapshots
		.iter()
		.map(|pod| pod["metadata"]["uid"].as_str().unwrap().to_owned())
		.collect()
}

fn nanos(value: &Value) -> i64 {
	DateTime::parse_from_rfc3339(value.as_str().unwrap())
		.unwrap()
		.timestamp_nanos_opt()
		.unwrap()
}

fn canonical(value: &Value) -> Value {
	match value {
		Value::Object(object) => {
			let sorted: BTreeMap<_, _> = object
				.iter()
				.map(|(key, value)| (key.clone(), canonical(value)))
				.collect();
			serde_json::to_value(sorted).unwrap()
		}
		Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
		value => value.clone(),
	}
}

fn validate_provider(directory: &Path, receipt: &Value) {
	let uid = receipt["allocation_id"].as_str().unwrap();
	assert_eq!(receipt["evidence"]["pod_uid"], uid);
	assert_eq!(receipt["evidence"]["start_time"], receipt["started_at"]);
	let name = format!("operation-{}", receipt["operation_id"].as_str().unwrap());
	let observations = provider(directory, &[name]);
	let expected: BTreeMap<String, i64> = receipt["evidence"]["containers"]
		.as_array()
		.unwrap()
		.iter()
		.map(|container| {
			(
				container["name"].as_str().unwrap().to_owned(),
				nanos(&container["finished_at"]),
			)
		})
		.collect();
	assert!(!expected.is_empty());
	assert!(
		observations.iter().any(|pod| {
			if pod["metadata"]["uid"] != uid
				|| pod["status"]["startTime"] != receipt["started_at"]
				|| pod["metadata"]["resourceVersion"].as_str().is_none()
			{
				return false;
			}
			let declared: BTreeSet<_> = ["containers", "initContainers"]
				.into_iter()
				.flat_map(|group| pod["spec"][group].as_array().unwrap().iter())
				.map(|item| item["name"].as_str().unwrap().to_owned())
				.collect();
			let mut stopped = BTreeMap::new();
			for item in ["containerStatuses", "initContainerStatuses"]
				.into_iter()
				.flat_map(|group| pod["status"][group].as_array().unwrap().iter())
			{
				if item["restartCount"].as_u64().unwrap_or(0) != 0 {
					return false;
				}
				let Some(finished) = item["state"]["terminated"].get("finishedAt") else {
					return false;
				};
				stopped.insert(item["name"].as_str().unwrap().to_owned(), nanos(finished));
			}
			declared == expected.keys().cloned().collect() && stopped == expected
		}),
		"the independent durable provider snapshot must contain every declared terminal container"
	);
	assert_eq!(
		*expected.values().max().unwrap(),
		nanos(&receipt["ended_at"])
	);
	let mut unsigned = receipt.clone();
	unsigned.as_object_mut().unwrap().remove("digest");
	assert_eq!(
		receipt["digest"],
		format!(
			"{:x}",
			Sha256::digest(serde_json::to_vec(&canonical(&unsigned)).unwrap())
		)
	);
}

fn validate_facts(receipt: &Value, tenant: &str, account: Uuid, saved: &Value) -> i64 {
	let raw = saved["raw"].as_array().unwrap();
	assert_eq!(raw.len(), 1);
	let raw = &raw[0];
	assert_eq!(raw["allocation"], receipt["allocation_id"]);
	assert_eq!(raw["tenant"], tenant);
	assert_eq!(receipt["tenant"], tenant);
	assert_eq!(raw["receipt_digest"], receipt["digest"]);
	let start = nanos(&receipt["started_at"]);
	let end = nanos(&receipt["ended_at"]);
	assert_eq!(raw["started_ns"], start);
	assert_eq!(raw["ended_ns"], end);
	let mut cursor = start;
	let (mut billable, mut exempt, mut covered) = (0_i64, 0_i64, 0_i64);
	for segment in saved["segments"].as_array().unwrap() {
		assert_eq!(segment["allocation"], receipt["allocation_id"]);
		assert_eq!(segment["tenant"], tenant);
		assert_eq!(segment["starts_ns"], cursor);
		let next = segment["ends_ns"].as_i64().unwrap();
		assert!(next > cursor);
		match segment["status"].as_str().unwrap() {
			"exempt" => exempt += next - cursor,
			"covered" => {
				assert_eq!(segment["account_id"], account.to_string());
				assert!(segment["coverage_id"].as_str().is_some());
				billable += next - cursor;
				covered += next - cursor;
			}
			"uncovered" => billable += next - cursor,
			other => panic!("unexpected frozen status {other}"),
		}
		cursor = next;
	}
	assert_eq!(cursor, end);
	assert_eq!(
		billable + exempt,
		end - start,
		"complete raw duration conserves both frozen categories"
	);
	assert_eq!(raw["billable_ns"], billable);
	assert_eq!(raw["millis"], billable / 1_000_000);
	covered
}

struct Dispatch {
	tenant: String,
	context: String,
	token: String,
	credential: Uuid,
	run: aidash_server::domain::Run,
	account: Uuid,
}

impl Dispatch {
	async fn new(d: &Cloud, alice: &Person) -> Self {
		let (status, created) = d
			.call(
				alice,
				"POST",
				"/api/tenants",
				None,
				json!({"label":"Real allocation acceptance"}),
			)
			.await;
		assert_eq!(status, 201);
		let tenant = created["name"].as_str().unwrap().to_owned();
		let (status, session) = d
			.call(alice, "GET", "/auth/session", None, Value::Null)
			.await;
		assert_eq!(status, 200);
		let mapping = session["mappings"]
			.as_array()
			.unwrap()
			.iter()
			.find(|mapping| mapping["tenant"] == tenant)
			.unwrap();
		assert_eq!(mapping["credential_active"], true);
		let subject = mapping["subject"].as_str().unwrap();
		let context = format!("mapping:{tenant}:{}", mapping["id"].as_str().unwrap());
		let account = d.account(alice, Some("team")).await;
		let ((a, _), (b, _)) = d.attach(alice, &context, alice, &tenant, account).await;
		assert!(matches!(a, 200 | 201 | 202) && matches!(b, 200 | 201 | 202));
		let f = &d.federation;
		let node = common::default_node(&f.store, &tenant).await;
		let policy_path = format!("/api/authorization/{tenant}");
		let (status, snapshot, ..) = d.send("GET", &policy_path, None, None, Value::Null).await;
		assert_eq!(status, 200);
		let mut policy = snapshot["bundle"].clone();
		let agent = aidash_server::domain::qualified_agent(&node, "allocation-agent", "1.0.0");
		policy["subjects"][&agent] = json!({"kind":"agent","node":node.node});
		policy["policies"].as_array_mut().unwrap().push(json!({
			"id":"allocation-acceptance-work", "effect":"allow", "subjects":{"ids":[agent]},
			"actions":["*"], "resources":{"kinds":["*"]}
		}));
		let (status, _, ..) = d
			.send(
				"POST",
				&policy_path,
				None,
				None,
				json!({"expected_revision":snapshot["revision"],"bundle":policy}),
			)
			.await;
		assert_eq!(status, 200);
		let name = aidash_domain::tenancy::TenantName::new(&tenant).unwrap();
		let bundles: Vec<_> = f
			.registry
			.list(
				&name,
				&aidash_server::registry::Search {
					kind: Some("bundle".into()),
					..Default::default()
				},
			)
			.await
			.unwrap()
			.into_iter()
			.filter(|entry| entry.tags.iter().any(|tag| tag == "sandboxed"))
			.collect();
		assert!(!bundles.is_empty());
		let bindings: Vec<_> = bundles.iter().map(|entry| json!({"kind":"bundle","target":{"registry_deployment":f.config.deployment,"id":entry.id,"version":entry.version},"narrow":{}})).collect();
		// The model is admitted but never invoked by this capability-only worker.
		f.store
			.keyless
			.permit_origin_of("http://127.0.0.1:1")
			.unwrap();
		for (kind, id, config) in [
			(
				"model",
				"allocation-model",
				json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
			),
			(
				"agent",
				"allocation-agent",
				json!({"schema_version":1,"model":{"id":"allocation-model","version":"1.0.0"},"instructions":"Model-neutral capability acceptance","bindings":bindings,"remove_default":[]}),
			),
		] {
			f.registry.register(&name, serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Acceptance"},"schema":{},"config":config})).unwrap()).await.unwrap();
		}
		let registered = f
			.registry
			.get(&name, "allocation-agent", "1.0.0")
			.await
			.unwrap();
		common::approve_agent_on(f, &tenant, &registered, &node).await;
		// A separate work credential shares the actual area-owning Subject with
		// the browser; revoking it must leave the browser's own credential live.
		let (status, credential) = d
			.call(
				alice,
				"POST",
				&format!("{policy_path}/credentials"),
				Some(&context),
				json!({"subject":subject}),
			)
			.await;
		assert_eq!(status, 200);
		assert_eq!(credential["credential"]["subject"], subject);
		assert_eq!(credential["credential"]["mapping_id"], mapping["id"]);
		let token = credential["token"].as_str().unwrap().to_owned();
		let credential = serde_json::from_value(credential["credential"]["id"].clone()).unwrap();
		let (status, workspace) = common::request(
			&d.app,
			&token,
			"POST",
			"/api/workspaces",
			json!({"title":"Allocation","goal":"Exercise real runtime","node":node.node}),
		)
		.await;
		assert_eq!(status, 200);
		let (status, task) = common::request(
			&d.app,
			&token,
			"POST",
			&format!(
				"/api/workspaces/{}/tasks",
				workspace["id"].as_str().unwrap()
			),
			json!({"title":"Allocation","description":"Native capability dispatch"}),
		)
		.await;
		assert_eq!(status, 200);
		let (status, _) = common::request(
			&d.app,
			&token,
			"POST",
			&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
			json!({"node_id":f.config.deployment,"agent":{"id":"allocation-agent","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200);
		let run = f
			.store
			.runs(&TenancyScope::tenant(&tenant).unwrap())
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id.to_string() == task["id"].as_str().unwrap())
			.unwrap();
		Self {
			tenant,
			context,
			token,
			credential,
			run,
			account,
		}
	}
	async fn call(&self, d: &Cloud, kind: &str, mut input: Value) -> Value {
		input["idempotency_key"] = json!(Uuid::new_v4());
		let (status, value) = common::request(
			&d.app,
			&self.token,
			"POST",
			&format!("/api/runs/{}/{kind}", self.run.id),
			input,
		)
		.await;
		assert_eq!(status, 200, "normal {kind} admission failed");
		value
	}
	async fn completed(
		&self,
		d: &Cloud,
		worker: &Worker,
		recorder: &Recorder,
		kind: &str,
		id: Uuid,
	) -> Value {
		eventually(
			worker,
			recorder,
			"ordinary operation completion",
			async || {
				let (status, value) = common::request(
					&d.app,
					&self.token,
					"POST",
					&format!("/api/runs/{}/{kind}/poll", self.run.id),
					json!({"operation_id":id}),
				)
				.await;
				assert_eq!(status, 200);
				assert!(!matches!(
					value["status"].as_str(),
					Some("failed" | "uncertain" | "withdrawn")
				));
				(value["status"] == "completed").then_some(value)
			},
		)
		.await
	}
}

async fn output_ack(
	d: &Cloud,
	dispatch: &Dispatch,
	worker: &Worker,
	recorder: &Recorder,
	evidence: &Evidence,
	id: Uuid,
) {
	eventually(
		worker,
		recorder,
		"ordinary native operation output ACK",
		async || {
			let row = operation(d, &dispatch.tenant, id).await;
			(row["result"]["runner_acknowledged"] == true && evidence.operation_ack(id))
				.then_some(())
		},
	)
	.await;
	evidence.record(json!({"kind":"operation_output_ack_committed","operation":id}));
}

async fn drain(
	d: &Cloud,
	dispatch: &Dispatch,
	worker: &Worker,
	recorder: &Recorder,
	evidence: &Evidence,
	provider_directory: &Path,
	uid: &str,
) -> (Value, Value) {
	let receipt = eventually(
		worker,
		recorder,
		"complete real provider receipt",
		async || evidence.receipt(uid, 0),
	)
	.await;
	validate_provider(provider_directory, &receipt);
	let ack = eventually(
		worker,
		recorder,
		"exact native allocation ACK",
		async || evidence.ack(uid, true),
	)
	.await;
	assert_eq!(
		ack["request"],
		json!({"instance":receipt["runner_instance"],"digest":receipt["digest"]})
	);
	assert_eq!(
		ack["response"],
		json!({"allocation_id":uid,"instance":receipt["runner_instance"],"digest":receipt["digest"],"acknowledged":true})
	);
	let saved = facts(d, uid).await;
	validate_facts(&receipt, &dispatch.tenant, dispatch.account, &saved);
	evidence.record(json!({"kind":"durable_facts","allocation":uid,"facts":saved}));
	(receipt, saved)
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "requires the owned admitted Enterprise allocation runtime selector"]
#[timeout(common::TEST_BOUND)]
async fn actual_allocation_receipts_reach_frozen_usage_before_independent_ack(
	#[from(isolated_runtime)] runtime: common::RuntimeFuture,
) {
	let profile = (*Runtime::from_env().unwrap().0).clone();
	let mut controller = Controller::owned(&profile);
	let provider_directory = controller.directory.join("allocation-evidence");
	let evidence = Evidence::new(&provider_directory);
	let health = controller.get("/v1/health").await.unwrap();
	assert_eq!(health["verified"], true);
	assert_eq!(health["python_verified"], true);
	let runner = profile.runner.as_ref().unwrap();
	assert_eq!(health["image"], runner.image);
	let instance = health["instance"].clone();
	evidence.record(
		json!({"kind":"initial_admission","health":health,"idle_seconds":profile.idle_seconds}),
	);
	let upstream: SocketAddr = controller
		.endpoint
		.strip_prefix("http://")
		.unwrap()
		.parse()
		.unwrap();
	assert!(upstream.ip().is_loopback());
	let recorder = Recorder::start(upstream, evidence.clone()).await;
	let mut observed_profile = profile.clone();
	observed_profile.runner.as_mut().unwrap().endpoint = recorder.endpoint.clone();
	let runtime = async move {
		let mut runtime = runtime.await;
		// Cloud::new subsequently clones this Store, installs the actual Meter,
		// and constructs its HTTP app. Its pre-installation fixture is never used.
		runtime.federation.store.capabilities = Runtime::new(observed_profile).unwrap();
		runtime
	}
	.boxed()
	.shared();
	let d = Cloud::new(runtime).await;
	let alice = d.person("allocation-owner").await;
	let dispatch = Dispatch::new(&d, &alice).await;
	let worker = Worker::start(&d);
	let mut allocations = Vec::new();

	// Normal HTTP shell dispatch is the first primary producer edge.
	let shell = dispatch
		.call(
			&d,
			"shell",
			json!({"expected_revision":1,"command":"printf 'allocation shell\\n'"}),
		)
		.await;
	let shell_id: Uuid = serde_json::from_value(shell["operation_id"].clone()).unwrap();
	let shell_done = dispatch
		.completed(&d, &worker, &recorder, "shell", shell_id)
		.await;
	output_ack(&d, &dispatch, &worker, &recorder, &evidence, shell_id).await;
	let shell_name = format!("operation-{shell_id}");
	let shell_uid = eventually(&worker, &recorder, "provider shell UID", async || {
		pod_uids(&provider(
			&provider_directory,
			std::slice::from_ref(&shell_name),
		))
		.pop_first()
	})
	.await;
	allocations.push(
		drain(
			&d,
			&dispatch,
			&worker,
			&recorder,
			&evidence,
			&provider_directory,
			&shell_uid,
		)
		.await,
	);

	// Two real cells share a live provider UID and preserve the Python heap.
	let first = dispatch
		.call(
			&d,
			"python",
			json!({"expected_revision":shell_done["revision"],"code":"counter = 41\nfrom pathlib import Path\nPath('allocation-saved.txt').write_text('saved through native idle')\nprint(counter)"}),
		)
		.await;
	let first_id: Uuid = serde_json::from_value(first["operation_id"].clone()).unwrap();
	let first_done = dispatch
		.completed(&d, &worker, &recorder, "python", first_id)
		.await;
	assert!(first_done["output"].as_str().unwrap().contains("41"));
	output_ack(&d, &dispatch, &worker, &recorder, &evidence, first_id).await;
	let first_name = format!("operation-{first_id}");
	let before_second = provider(&provider_directory, std::slice::from_ref(&first_name));
	let first_uids = pod_uids(&before_second);
	assert_eq!(first_uids.len(), 1);
	let reused_uid = first_uids.first().unwrap().clone();
	let second = dispatch.call(&d, "python", json!({"expected_revision":first_done["revision"],"expected_session_id":first_done["session_id"],"code":"counter += 1\nprint(counter)"})).await;
	let second_id: Uuid = serde_json::from_value(second["operation_id"].clone()).unwrap();
	let second_done = dispatch
		.completed(&d, &worker, &recorder, "python", second_id)
		.await;
	assert_eq!(second_done["session_id"], first_done["session_id"]);
	assert!(second_done["output"].as_str().unwrap().contains("42"));
	output_ack(&d, &dispatch, &worker, &recorder, &evidence, second_id).await;
	let after_second = provider(
		&provider_directory,
		&[first_name.clone(), format!("operation-{second_id}")],
	);
	assert!(
		after_second.len() > before_second.len(),
		"second cell must cause fresh real provider observation"
	);
	assert_eq!(
		pod_uids(&after_second),
		first_uids,
		"session identity alone is insufficient: the actual provider UID is reused"
	);
	assert!(after_second.iter().any(|pod| {
		pod["status"]["containerStatuses"]
			.as_array()
			.unwrap()
			.iter()
			.any(|state| state["name"] == "execution" && state["state"].get("running").is_some())
	}));
	assert_eq!(
		facts(&d, &reused_uid).await,
		json!({"raw":[],"segments":[]}),
		"cell completion cannot create a partial allocation fact"
	);
	evidence.record(json!({"kind":"python_reuse","operations":[first_id,second_id],"allocation":reused_uid,"no_early_fact":true}));

	// Acquire the real capacity barrier before the provider's normal idle stop.
	// No authority transition is attempted while this lock is held.
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &dispatch.tenant)
		.await
		.unwrap();
	let pid: Vec<Value> = store::fetch(
		&mut *held,
		&Query::select()
			.expr_as(Expr::cust("pg_backend_pid()"), Alias::new("pid"))
			.to_owned(),
	)
	.await
	.unwrap();
	let pid = i32::try_from(pid[0]["pid"].as_i64().unwrap()).unwrap();
	let retained = eventually(
		&worker,
		&recorder,
		"normal idle produces a complete native-listed receipt",
		async || evidence.receipt(&reused_uid, 0),
	)
	.await;
	validate_provider(&provider_directory, &retained);
	let idle = eventually(
		&worker,
		&recorder,
		"normal Python maintenance confirms actual idle expiration",
		async || idle_reset(&d, &dispatch.tenant, &second_done["session_id"]).await,
	)
	.await;
	evidence.record(json!({"kind":"actual_idle_stop","allocation":reused_uid,"session":idle}));
	common::wait_for_blocked(d.control(), pid, 1).await;
	worker.live(&recorder);
	assert_eq!(
		facts(&d, &reused_uid).await,
		json!({"raw":[],"segments":[]})
	);
	assert!(evidence.ack(&reused_uid, true).is_none());
	let pending = controller
		.get("/v1/allocations/receipts?limit=64")
		.await
		.unwrap();
	assert_eq!(pending["instance"], instance);
	assert!(pending["receipts"].as_array().unwrap().contains(&retained));
	evidence.record(json!({"kind":"blocked_meter_pending_receipt","allocation":reused_uid,"page":pending,"raw":[],"segments":[]}));

	controller.stop_initial().await;
	evidence.record(json!({"kind":"owned_controller_unavailable","allocation":reused_uid}));
	held.commit().await.unwrap();
	let committed = eventually(
		&worker,
		&recorder,
		"actual Meter commits while controller is unavailable",
		async || {
			let facts = facts(&d, &reused_uid).await;
			(facts["raw"].as_array().unwrap().len() == 1).then_some(facts)
		},
	)
	.await;
	validate_facts(&retained, &dispatch.tenant, dispatch.account, &committed);
	eventually(
		&worker,
		&recorder,
		"native exact ACK fails against the stopped controller",
		async || evidence.ack(&reused_uid, false),
	)
	.await;
	assert!(evidence.ack(&reused_uid, true).is_none());
	let frozen_bytes = serde_json::to_vec(&committed).unwrap();
	evidence.record(
		json!({"kind":"committed_before_failed_ack","allocation":reused_uid,"facts":committed}),
	);
	let replay_start = evidence.events().len();
	controller
		.restart(&runner.image, &instance, &evidence)
		.await;
	let replayed = eventually(
		&worker,
		&recorder,
		"same-journal native re-delivery",
		async || evidence.receipt(&reused_uid, replay_start),
	)
	.await;
	assert_eq!(replayed, retained);
	let drained = drain(
		&d,
		&dispatch,
		&worker,
		&recorder,
		&evidence,
		&provider_directory,
		&reused_uid,
	)
	.await;
	assert_eq!(
		serde_json::to_vec(&drained.1).unwrap(),
		frozen_bytes,
		"duplicate native intake preserves every frozen row and identifier byte-exactly"
	);
	allocations.push(drained);
	let pin = rows(
		&d,
		&TenancyScope::NoTenant,
		select(
			"runner_allocation_journals",
			&["deployment", "runner_instance"],
		)
		.and_where(Expr::col("deployment").eq(Expr::value(d.federation.store.deployment.as_str())))
		.to_owned(),
	)
	.await;
	assert_eq!(pin.len(), 1);
	assert_eq!(pin[0]["runner_instance"], instance);
	evidence.record(json!({"kind":"deployment_runner_pin","pin":pin}));

	// The ordinary reset acknowledgement creates a new actual allocation.
	let input = json!({"expected_revision":second_done["revision"],"expected_session_id":second_done["session_id"],"code":"assert 'counter' not in globals()\nfrom pathlib import Path\nsaved = Path('allocation-saved.txt').read_text()\nassert saved == 'saved through native idle'\nprint(saved)\nprint('new interpreter')"});
	let reset = dispatch.call(&d, "python", input.clone()).await;
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	assert_ne!(reset["session_id"], second_done["session_id"]);
	let mut accepted = input;
	accepted["expected_session_id"] = reset["session_id"].clone();
	let third = dispatch.call(&d, "python", accepted).await;
	let third_id: Uuid = serde_json::from_value(third["operation_id"].clone()).unwrap();
	let third_done = dispatch
		.completed(&d, &worker, &recorder, "python", third_id)
		.await;
	assert!(
		third_done["output"]
			.as_str()
			.unwrap()
			.contains("saved through native idle")
	);
	output_ack(&d, &dispatch, &worker, &recorder, &evidence, third_id).await;
	let third_name = format!("operation-{third_id}");
	let third_uids = pod_uids(&provider(
		&provider_directory,
		std::slice::from_ref(&third_name),
	));
	assert_eq!(third_uids.len(), 1);
	let third_uid = third_uids.first().unwrap().clone();
	assert_ne!(third_uid, reused_uid);
	evidence.record(json!({"kind":"python_recreated","operation":third_id,"allocation":third_uid,"previous_allocation":reused_uid,"saved_output":third_done["output"]}));
	allocations.push(
		drain(
			&d,
			&dispatch,
			&worker,
			&recorder,
			&evidence,
			&provider_directory,
			&third_uid,
		)
		.await,
	);
	let idle = eventually(
		&worker,
		&recorder,
		"recreated Python allocation also stops by actual idle",
		async || idle_reset(&d, &dispatch.tenant, &third_done["session_id"]).await,
	)
	.await;
	evidence.record(json!({"kind":"actual_idle_stop","allocation":third_uid,"session":idle}));

	// A distinct authority-loss phase: revoke an actual originating credential
	// while its normally dispatched shell is still running. Retain the last
	// genuine receipt behind the same real capacity barrier through Run terminal.
	let authority_dispatched = tokio::time::Instant::now();
	let authority = dispatch.call(&d, "shell", json!({"expected_revision":third_done["revision"],"command":"printf 'authority active\\n'; sleep 110"})).await;
	let authority_id: Uuid = serde_json::from_value(authority["operation_id"].clone()).unwrap();
	let authority_name = format!("operation-{authority_id}");
	let authority_uid = eventually(
		&worker,
		&recorder,
		"authority-loss allocation is really running",
		async || controller.running_uid(runner, &authority_name).await,
	)
	.await;
	let authority_native = eventually(
		&worker,
		&recorder,
		"authority-loss command is executing in the native operation",
		async || {
			let native = controller
				.get(&format!("/v1/operations/{authority_id}"))
				.await
				.unwrap();
			assert_eq!(native["operation_id"], authority_id.to_string());
			(native["status"] == "running" && native["executed"] == true).then_some(native)
		},
	)
	.await;
	let authority_binding = only(
		&authority_native,
		&["operation_id", "digest", "area_id", "epoch"],
	);
	assert!(
		authority_dispatched.elapsed() < Wait::from_secs(60),
		"authority loss must precede the real shell's natural/deadline completion"
	);
	assert!(evidence.receipt(&authority_uid, 0).is_none());
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &dispatch.tenant)
		.await
		.unwrap();
	let pid: Vec<Value> = store::fetch(
		&mut *held,
		&Query::select()
			.expr_as(Expr::cust("pg_backend_pid()"), Alias::new("pid"))
			.to_owned(),
	)
	.await
	.unwrap();
	let pid = i32::try_from(pid[0]["pid"].as_i64().unwrap()).unwrap();
	let authority_start = evidence.record(json!({"kind":"authority_loss_requested","allocation":authority_uid,"credential":dispatch.credential}));
	let (status, revoked, ..) = d
		.send(
			"POST",
			&format!(
				"/api/authorization/{}/credentials/{}/revoke",
				dispatch.tenant, dispatch.credential
			),
			None,
			None,
			json!({}),
		)
		.await;
	assert_eq!(status, 200);
	assert!(
		authority_dispatched.elapsed() < Wait::from_secs(100),
		"the shell's natural deadline must still be future when real revocation commits"
	);
	let revoked_at = &revoked["revoked_at"];
	assert!(revoked_at.as_str().is_some());
	evidence.record(
		json!({"kind":"authority_revoked","credential":dispatch.credential,"revoked_at":revoked_at}),
	);
	let (status, _) = common::request(
		&d.app,
		&dispatch.token,
		"POST",
		&format!("/api/runs/{}/shell/poll", dispatch.run.id),
		json!({"operation_id":authority_id}),
	)
	.await;
	assert_eq!(
		status, 401,
		"the actual originating credential has no authority"
	);
	let stopped = eventually(
		&worker,
		&recorder,
		"authority withdrawal requests cancellation and native writer stops",
		async || {
			let row = operation(&d, &dispatch.tenant, authority_id).await;
			let native = controller
				.get(&format!("/v1/operations/{authority_id}"))
				.await
				.unwrap();
			assert_eq!(only(&native, &["operation_id", "digest", "area_id", "epoch"]), authority_binding);
			(matches!(row["state"].as_str(), Some("cancelling" | "withdrawn"))
				&& row["result"]["error"] == "AUTHORITY_WITHDRAWN"
				&& native["executed"] == true
				&& native["cancel_requested"] == true
				&& native["termination_confirmed"] == true)
				.then(|| json!({"binding":authority_binding,"state":row["state"],"native":only(&native, &["status", "executed", "cancel_requested", "termination_confirmed"])}))
		},
	)
	.await;
	assert!(
		authority_dispatched.elapsed() < Wait::from_secs(100),
		"positive authority stop must precede the real shell's natural/deadline completion"
	);
	evidence
		.record(json!({"kind":"authority_native_stop","allocation":authority_uid,"proof":stopped}));
	let authority_receipt = eventually(
		&worker,
		&recorder,
		"allocation intake survives credential loss",
		async || evidence.receipt(&authority_uid, authority_start),
	)
	.await;
	validate_provider(&provider_directory, &authority_receipt);
	// Kubernetes metav1.Time can have only whole-second precision. Keep its
	// exact value for receipt/DB equality; the committed revoke, rejected bearer
	// and withdrawn/confirmed-stop transition establish authority ordering.
	let revoked_second = DateTime::parse_from_rfc3339(revoked_at.as_str().unwrap())
		.unwrap()
		.timestamp();
	assert!(
		nanos(&authority_receipt["ended_at"]) >= revoked_second * 1_000_000_000,
		"provider stop cannot predate the second containing committed revocation"
	);
	common::wait_for_blocked(d.control(), pid, 1).await;
	worker.live(&recorder);
	let pending = controller
		.get("/v1/allocations/receipts?limit=64")
		.await
		.unwrap();
	assert_eq!(pending["instance"], instance);
	assert!(
		pending["receipts"]
			.as_array()
			.unwrap()
			.contains(&authority_receipt)
	);
	assert_eq!(
		facts(&d, &authority_uid).await,
		json!({"raw":[],"segments":[]})
	);
	assert!(evidence.ack(&authority_uid, true).is_none());
	evidence.record(json!({"kind":"authority_meter_blocked_before_terminal","allocation":authority_uid,"page":pending,"raw":[],"segments":[]}));
	// The capability loop waits in Meter before its next withdrawal/output ACK.
	// Normal Harness cancellation owns Run settlement independently of that loop.
	let (status, _) = d
		.call(
			&alice,
			"POST",
			&format!("/api/runs/{}/control", dispatch.run.id),
			Some(&dispatch.context),
			json!({"action":"cancel"}),
		)
		.await;
	assert_eq!(status, 200);
	assert!(
		aidash_server::harness::Harness {
			federation: d.federation.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	let run = d
		.federation
		.store
		.runs(&TenancyScope::tenant(&dispatch.tenant).unwrap())
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.id == dispatch.run.id)
		.unwrap();
	assert!(run.state.phase().is_terminal());
	evidence.record(json!({"kind":"run_terminal","run":run.id,"phase":run.state.phase().as_str()}));
	let pending = controller
		.get("/v1/allocations/receipts?limit=64")
		.await
		.unwrap();
	assert_eq!(pending["instance"], instance);
	assert!(
		pending["receipts"]
			.as_array()
			.unwrap()
			.contains(&authority_receipt)
	);
	assert_eq!(
		facts(&d, &authority_uid).await,
		json!({"raw":[],"segments":[]})
	);
	assert!(evidence.ack(&authority_uid, true).is_none());
	evidence.record(json!({"kind":"terminal_before_first_meter_commit","run":run.id,"allocation":authority_uid,"page":pending,"raw":[],"segments":[]}));
	held.commit().await.unwrap();
	evidence.record(
		json!({"kind":"terminal_capacity_released","run":run.id,"allocation":authority_uid}),
	);
	eventually(
		&worker,
		&recorder,
		"normal withdrawal persists actual termination after Meter release",
		async || {
			let row = operation(&d, &dispatch.tenant, authority_id).await;
			(row["state"] == "withdrawn" && row["result"]["termination_confirmed"] == true)
				.then_some(())
		},
	)
	.await;
	output_ack(&d, &dispatch, &worker, &recorder, &evidence, authority_id).await;
	allocations.push(
		drain(
			&d,
			&dispatch,
			&worker,
			&recorder,
			&evidence,
			&provider_directory,
			&authority_uid,
		)
		.await,
	);
	eventually(
		&worker,
		&recorder,
		"no active operation worklist remains",
		async || {
			let active = rows(
				&d,
				&TenancyScope::tenant(&dispatch.tenant).unwrap(),
				select("core_operations", &["id"])
					.and_where(Expr::col("state").is_in([
						"prepared",
						"submitted",
						"running",
						"cancelling",
					]))
					.to_owned(),
			)
			.await;
			active.is_empty().then_some(())
		},
	)
	.await;
	let final_after = evidence.events().len();
	eventually(
		&worker,
		&recorder,
		"observed empty allocation page after terminal Run and empty worklist",
		async || {
			evidence
				.events()
				.iter()
				.skip(final_after)
				.any(|event| {
					event["kind"] == "allocation_page"
						&& event["response"]["receipts"] == json!([])
						&& event["response"]["pending_evidence"] == 0
				})
				.then_some(())
		},
	)
	.await;
	let pending = controller
		.get("/v1/allocations/receipts?limit=64")
		.await
		.unwrap();
	assert_eq!(pending["receipts"], json!([]));
	assert_eq!(pending["pending_evidence"], 0);
	evidence.record(json!({"kind":"final_allocation_drain","page":pending}));
	let owned_uids: BTreeSet<_> = [&shell_uid, &reused_uid, &third_uid, &authority_uid]
		.into_iter()
		.cloned()
		.collect();
	eventually(
		&worker,
		&recorder,
		"controller releases every captured owned Pod normally",
		async || {
			controller
				.remaining_pods(runner, &owned_uids)
				.await
				.is_empty()
				.then_some(())
		},
	)
	.await;
	evidence
		.record(json!({"kind":"owned_provider_cleanup","allocations":owned_uids,"remaining":[]}));

	// The authorized account-period query must agree with the independently
	// conserved frozen rows for these actual receipts. Historical move/retention
	// cases remain owned by their existing deterministic billing tests.
	let start = allocations
		.iter()
		.map(|(receipt, _)| nanos(&receipt["started_at"]))
		.min()
		.unwrap();
	let end = allocations
		.iter()
		.map(|(receipt, _)| nanos(&receipt["ended_at"]))
		.max()
		.unwrap();
	let expected: i64 = allocations
		.iter()
		.map(|(receipt, saved)| validate_facts(receipt, &dispatch.tenant, dispatch.account, saved))
		.sum();
	assert!(
		expected > 0,
		"the real covered allocation interval must be nonzero"
	);
	let path = format!(
		"/api/billing/accounts/{}/usage?starts_at={}&ends_at={}",
		dispatch.account,
		DateTime::<Utc>::from_timestamp_nanos(start)
			.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
		DateTime::<Utc>::from_timestamp_nanos(end)
			.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
	);
	let (status, usage) = d.call(&alice, "GET", &path, None, Value::Null).await;
	assert_eq!(status, 200);
	assert_eq!(usage["pod_nanos"], expected.to_string());
	let reported: BTreeSet<_> = usage["segments"]
		.as_array()
		.unwrap()
		.iter()
		.map(|segment| segment["allocation"].as_str().unwrap().to_owned())
		.collect();
	assert_eq!(
		reported,
		[shell_uid, reused_uid, third_uid, authority_uid]
			.into_iter()
			.collect()
	);
	evidence.record(json!({"kind":"authorized_account_period","usage":usage}));
	worker.live(&recorder);
	worker.finish().await;
	controller.finish().await;
	drop(recorder);
	d.finish().await;
}
