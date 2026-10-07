use super::*;
use crate::ports::federation::{DelegationRetries, PeerReply};
use aidash_domain::{Task, federation::Peer};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::BTreeMap,
	sync::{
		Mutex,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
};

struct Repository {
	peers: Vec<Peer>,
	pages: BTreeMap<u64, Value>,
	task: Task,
	scoped: bool,
	reserve_conflict: bool,
	reservations: AtomicUsize,
	pinned: Mutex<Option<aidash_domain::registry::bindings::ForeignAgentSnapshot>>,
	held: Arc<AtomicBool>,
	pending: Vec<Delegation>,
}
struct RetryBatch {
	pending: Vec<Delegation>,
	held: Arc<AtomicBool>,
}
impl DelegationRetries for RetryBatch {
	fn pending(&self) -> &[Delegation] {
		&self.pending
	}
}
impl Drop for RetryBatch {
	fn drop(&mut self) {
		self.held.store(false, Ordering::SeqCst);
	}
}
#[async_trait]
impl FederationRepository for Repository {
	async fn peers(&self) -> Result<Vec<Peer>> {
		Ok(self.peers.clone())
	}
	async fn peer(&self, node: &str) -> Result<Peer> {
		Ok(peer(node))
	}
	async fn task(&self, _id: Uuid) -> Result<Task> {
		Ok(self.task.clone())
	}
	async fn require_legacy_workspace(&self, _workspace: Uuid) -> Result<()> {
		if self.scoped {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn require_legacy_agent(&self, _agent: &EntityRef) -> Result<()> {
		Ok(())
	}
	async fn agent(&self, _agent: &EntityRef) -> Result<Entry> {
		Ok(entry("local"))
	}
	async fn agents(&self, search: &Search, offset: u64) -> Result<AgentPage> {
		assert_eq!(search.kind.as_deref(), Some("agent"));
		Ok(serde_json::from_value(self.pages[&offset].clone())?)
	}
	async fn reserve_delegation(
		&self,
		_expected: &Task,
		_node: &str,
		_agent: &EntityRef,
	) -> Result<Delegation> {
		self.reservations.fetch_add(1, Ordering::SeqCst);
		if self.reserve_conflict {
			Err(Error::Conflict("task revision changed".into()))
		} else {
			Ok(self.pending[0].clone())
		}
	}
	async fn reserve_pinned_delegation(
		&self,
		expected: &Task,
		node: &str,
		agent: &EntityRef,
		snapshot: &aidash_domain::registry::bindings::ForeignAgentSnapshot,
	) -> Result<Delegation> {
		{
			let mut saved = self.pinned.lock().unwrap();
			if saved.as_ref().is_some_and(|original| original != snapshot) {
				return Err(Error::Conflict("original receiver closure changed".into()));
			}
			*saved = Some(snapshot.clone());
		}
		self.reserve_delegation(expected, node, agent).await
	}
	async fn delegation_snapshot(
		&self,
		_: Uuid,
	) -> Result<Option<aidash_domain::registry::bindings::ForeignAgentSnapshot>> {
		Ok(self.pinned.lock().unwrap().clone())
	}
	async fn accept_local_run(&self, _task: &Task, _agent: &EntityRef) -> Result<Run> {
		Err(Error::Invalid("unexpected local delivery".into()))
	}
	async fn mark_delivered(&self, _task: Uuid) -> Result<()> {
		Ok(())
	}
	async fn claim_retries(&self) -> Result<Box<dyn DelegationRetries>> {
		self.held.store(true, Ordering::SeqCst);
		Ok(Box::new(RetryBatch {
			pending: self.pending.clone(),
			held: self.held.clone(),
		}))
	}
}
struct Transport {
	status: u16,
	transaction_pending: bool,
	run_message_pending: bool,
	pages: BTreeMap<(String, String), Value>,
	calls: Mutex<Vec<(String, String)>>,
	offers: Mutex<Vec<Value>>,
	pending: bool,
	held: Option<Arc<AtomicBool>>,
}
#[async_trait]
impl PeerTransport for Transport {
	async fn request(
		&self,
		peer: &Peer,
		_method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply> {
		if let Some(held) = &self.held {
			assert!(
				held.load(Ordering::SeqCst),
				"retry visibility was released before delivery"
			);
		}
		self.calls
			.lock()
			.unwrap()
			.push((peer.node_id.clone(), path.into()));
		if path == "/offers" {
			self.offers.lock().unwrap().push(body.unwrap().clone());
		}
		if self.pending && path == "/offers" {
			return std::future::pending().await;
		}
		if peer.node_id == "aidash://offline" {
			return Err(Error::External("peer unavailable".into()));
		}
		Ok(PeerReply {
			status: self.status,
			status_label: self.status.to_string(),
			transaction_pending: self.transaction_pending,
			run_message_pending: self.run_message_pending,
			body: self
				.pages
				.get(&(peer.node_id.clone(), path.into()))
				.cloned()
				.unwrap_or(Value::Null),
		})
	}
}
fn peer(node: &str) -> Peer {
	Peer {
		node_id: node.into(),
		endpoint: "http://127.0.0.1:1".into(),
		credential_env: "FIXTURE".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	}
}
fn entry(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},"description":{},"config":{"model":{"id":"model","version":"1.0.0"}}})).unwrap()
}
#[fixture]
fn repository() -> Repository {
	let task = Task {
		id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		title: "delegate".into(),
		description: String::new(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "fixture".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 0,
		created_at: chrono::Utc::now(),
	};
	let delegation = Delegation {
		task_id: task.id,
		node_id: "aidash://remote".into(),
		agent_id: "remote".into(),
		agent_version: "1.0.0".into(),
		delivered: false,
	};
	Repository {
		peers: vec![],
		pages: BTreeMap::from([(0, json!({"entries":[],"next_offset":null}))]),
		task,
		scoped: false,
		reserve_conflict: false,
		reservations: AtomicUsize::new(0),
		pinned: Mutex::new(None),
		held: Arc::new(AtomicBool::new(false)),
		pending: vec![delegation],
	}
}
#[fixture]
fn transport() -> Transport {
	Transport {
		status: 200,
		transaction_pending: false,
		run_message_pending: false,
		pages: BTreeMap::new(),
		calls: Mutex::new(vec![]),
		offers: Mutex::new(vec![]),
		pending: false,
		held: None,
	}
}
fn federation(repository: Arc<Repository>, transport: Arc<Transport>) -> Federation {
	Federation::new("aidash://local".into(), repository, transport)
}

#[rstest]
#[case(503, true, false, "pending")]
#[case(503, false, false, "external")]
#[case(409, false, true, "pending")]
#[case(409, false, false, "conflict")]
#[case(400, false, false, "external")]
#[case(401, false, false, "external")]
#[case(200, false, false, "success")]
#[tokio::test]
async fn peer_status_and_protocol_headers_preserve_retry_classification(
	repository: Repository,
	mut transport: Transport,
	#[case] status: u16,
	#[case] transaction_pending: bool,
	#[case] run_message_pending: bool,
	#[case] expected: &str,
) {
	// Arrange
	transport.status = status;
	transport.transaction_pending = transaction_pending;
	transport.run_message_pending = run_message_pending;
	let f = federation(Arc::new(repository), Arc::new(transport));
	// Act
	let result = f
		.request::<Value>("aidash://remote", "GET", "/task", None)
		.await;
	// Assert
	let classification = match result {
		Ok(Value::Null) => "success",
		Err(Error::TransactionPending) => "pending",
		Err(Error::Conflict(message)) => {
			assert_eq!(message, "remote task state changed");
			"conflict"
		}
		Err(Error::External(_)) => "external",
		other => panic!("unexpected reply: {other:?}"),
	};
	assert_eq!(classification, expected);
}

#[rstest]
#[case("/workspace",Some(json!({"operation":"workspace_record_chunk"})),true)]
#[case("/workspace",Some(json!({"operation":"workspace_read"})),false)]
#[case("/task",Some(json!({"operation":"workspace_record_chunk"})),false)]
#[case("/workspace", None, false)]
fn only_workspace_chunk_errors_keep_the_peer_validation_reason(
	#[case] path: &str,
	#[case] request: Option<Value>,
	#[case] mapped: bool,
) {
	let error = map_workspace_chunk_bad_request(
		path,
		request.as_ref(),
		&json!({"error":"chunk limit exceeded"}),
	);
	assert_eq!(error.is_some(), mapped);
	if mapped {
		assert!(matches!(error,Some(Error::Invalid(message)) if message == "chunk limit exceeded"));
	}
}

#[rstest]
#[tokio::test]
async fn discovery_reads_all_pages_and_keeps_other_peers_when_one_fails(
	mut repository: Repository,
	mut transport: Transport,
) {
	// Arrange
	repository.pages = BTreeMap::from([
		(
			0,
			json!({"entries":[entry("local-first")],"next_offset":100}),
		),
		(
			100,
			json!({"entries":[entry("local-last")],"next_offset":null}),
		),
	]);
	let mut disabled = peer("aidash://disabled");
	disabled.enabled = false;
	repository.peers = vec![peer("aidash://remote"), peer("aidash://offline"), disabled];
	transport.pages = BTreeMap::from([
		(
			("aidash://remote".into(), "/discover?offset=0".into()),
			json!({"entries":[entry("remote-first")],"next_offset":50}),
		),
		(
			("aidash://remote".into(), "/discover?offset=50".into()),
			json!({"entries":[entry("remote-last")],"next_offset":null}),
		),
	]);
	let transport = Arc::new(transport);
	let f = federation(Arc::new(repository), transport.clone());
	// Act
	let result = f.discover(&Search::default()).await.unwrap();
	// Assert
	assert_eq!(
		result
			.agents
			.iter()
			.map(|agent| agent.entity.id.as_str())
			.collect::<Vec<_>>(),
		["local-first", "local-last", "remote-first", "remote-last"]
	);
	assert_eq!(result.errors.len(), 1);
	assert_eq!(result.errors[0].node_id, "aidash://offline");
	assert_eq!(transport.calls.lock().unwrap().len(), 3);
}
#[rstest]
#[tokio::test]
async fn a_nonadvancing_discovery_cursor_fails_instead_of_looping(
	mut repository: Repository,
	transport: Transport,
) {
	repository
		.pages
		.insert(0, json!({"entries":[],"next_offset":0}));
	let result = federation(Arc::new(repository), Arc::new(transport))
		.discover(&Search::default())
		.await;
	assert!(
		matches!(result,Err(Error::External(message)) if message == "discovery cursor did not advance")
	);
}
#[rstest]
#[tokio::test]
async fn scoped_workspace_denial_precedes_peer_calls_and_reservation(
	mut repository: Repository,
	transport: Transport,
) {
	repository.scoped = true;
	let id = repository.task.id;
	let repository = Arc::new(repository);
	let transport = Arc::new(transport);
	let result = federation(repository.clone(), transport.clone())
		.delegate(
			id,
			"aidash://remote",
			&EntityRef {
				id: "remote".into(),
				version: "1.0.0".into(),
			},
		)
		.await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(repository.reservations.load(Ordering::SeqCst), 0);
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn reservation_revision_failure_never_delivers_an_offer(
	mut repository: Repository,
	mut transport: Transport,
) {
	repository.reserve_conflict = true;
	let id = repository.task.id;
	transport.pages.insert(
		("aidash://remote".into(), "/discover/remote/1.0.0".into()),
		json!(entry("remote")),
	);
	let repository = Arc::new(repository);
	let transport = Arc::new(transport);
	let result = federation(repository.clone(), transport.clone())
		.delegate(
			id,
			"aidash://remote",
			&EntityRef {
				id: "remote".into(),
				version: "1.0.0".into(),
			},
		)
		.await;
	assert!(matches!(result,Err(Error::Conflict(message)) if message == "task revision changed"));
	assert_eq!(repository.reservations.load(Ordering::SeqCst), 1);
	assert_eq!(
		*transport.calls.lock().unwrap(),
		[("aidash://remote".into(), "/discover/remote/1.0.0".into())]
	);
}
#[rstest]
#[tokio::test]
async fn retry_keeps_visibility_until_failed_delivery_finishes(
	repository: Repository,
	mut transport: Transport,
) {
	transport.status = 503;
	transport.held = Some(repository.held.clone());
	let held = repository.held.clone();
	let transport = Arc::new(transport);
	let f = federation(Arc::new(repository), transport.clone());
	f.retry_deliveries().await.unwrap();
	assert_eq!(
		*transport.calls.lock().unwrap(),
		[("aidash://remote".into(), "/offers".into())]
	);
	assert!(!held.load(Ordering::SeqCst));
}
#[rstest]
#[tokio::test]
async fn cancelled_retry_releases_its_owned_visibility_lease(
	repository: Repository,
	mut transport: Transport,
) {
	transport.pending = true;
	transport.held = Some(repository.held.clone());
	let held = repository.held.clone();
	let f = federation(Arc::new(repository), Arc::new(transport));
	let mut work = Box::pin(f.retry_deliveries());
	assert!(futures_util::poll!(work.as_mut()).is_pending());
	assert!(held.load(Ordering::SeqCst));
	drop(work);
	assert!(!held.load(Ordering::SeqCst));
}

#[rstest]
#[tokio::test]
async fn malformed_peer_success_is_an_external_failure(
	repository: Repository,
	transport: Transport,
) {
	let result = federation(Arc::new(repository), Arc::new(transport))
		.request::<Entry>("aidash://remote", "GET", "/task", None)
		.await;
	assert!(
		matches!(result,Err(Error::External(message)) if message.starts_with("invalid response JSON:"))
	);
}

async fn receiver_closure() -> aidash_domain::registry::bindings::ForeignAgentSnapshot {
	use crate::ports::bindings::{BindingCatalog, ProviderCatalog};
	use aidash_domain::{
		registry::bindings::*,
		tool::{ToolContract, providers::*},
	};
	struct Catalog(Vec<Entry>);
	#[async_trait]
	impl BindingCatalog for Catalog {
		async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
			self.0
				.iter()
				.find(|entry| entry.id == reference.id && entry.version == reference.version)
				.cloned()
				.ok_or(Error::Forbidden)
		}
		async fn installation(&mut self, _: &aidash_domain::registry::Projection) -> Result<()> {
			Err(Error::Forbidden)
		}
		async fn source(&mut self, _: &Entry) -> Result<()> {
			Err(Error::Forbidden)
		}
	}
	struct Providers;
	impl ProviderCatalog for Providers {
		fn contract(
			&self,
			descriptor: &ToolDescriptor,
			identity: &QualifiedRef,
		) -> Result<ToolContract> {
			Ok(descriptor.declared_contract(identity.clone())?)
		}
		fn implementation(&self, _: &ToolDescriptor) -> Result<String> {
			Ok("fixture-implementation".into())
		}
	}
	let node = "aidash://remote";
	let mut root = entry("remote");
	root.config["schema_version"] = json!(1);
	root.config["instructions"] = json!("Pinned public delegation");
	let mut model = entry("model");
	model.kind = "model".into();
	model.config = json!({});
	let mut catalog = Catalog(vec![model]);
	for operation in REQUIRED_TOOLS.iter().chain(DEFAULT_TOOLS) {
		let descriptor = core_descriptor(node, operation).unwrap();
		let mut tool = entry(&QualifiedRef::builtin(node, operation).id);
		tool.kind = "tool".into();
		tool.config = serde_json::to_value(descriptor).unwrap();
		catalog.0.push(tool);
	}
	let snapshot = crate::registry::bindings::resolve(
		&mut catalog,
		&Providers,
		QualifiedRef {
			registry_node: node.into(),
			id: root.id.clone(),
			version: root.version.clone(),
		},
		&root,
		true,
	)
	.await
	.unwrap();
	ForeignAgentSnapshot::from_snapshot(snapshot).unwrap()
}

#[rstest]
#[tokio::test]
async fn failed_agent_tool_delivery_and_restarted_retries_keep_the_original_receiver_closure(
	repository: Repository,
	mut transport: Transport,
) {
	transport.status = 503;
	transport.transaction_pending = true;
	let repository = Arc::new(repository);
	let transport = Arc::new(transport);
	let snapshot = receiver_closure().await;
	let task = repository.task.id;
	let federation = federation(repository.clone(), transport.clone());
	let reserved = federation
		.delegate_pinned(
			task,
			&snapshot.agent.registry_node,
			&snapshot.agent.local(),
			&snapshot,
		)
		.await
		.unwrap();
	assert!(!reserved.delivered);
	let restarted = Federation::new(
		"aidash://local".into(),
		repository.clone(),
		transport.clone(),
	);
	restarted.retry_deliveries().await.unwrap();
	let offers = transport.offers.lock().unwrap().clone();
	assert_eq!(offers.len(), 2);
	assert_eq!(offers[0], offers[1]);
	assert_eq!(
		offers[1]["binding_snapshot"],
		serde_json::to_value(&snapshot).unwrap()
	);
	assert!(
		transport
			.calls
			.lock()
			.unwrap()
			.iter()
			.all(|(_, path)| path == "/offers"),
		"retry must not discover another closure"
	);
	assert!(matches!(
		federation
			.delegate_pinned(task, "aidash://other", &snapshot.agent.local(), &snapshot)
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.reservations.load(Ordering::SeqCst), 1);
	let mut changed = snapshot;
	let root = changed
		.definitions
		.iter_mut()
		.find(|definition| definition.identity == changed.agent)
		.unwrap();
	root.definition.name.insert("en".into(), "changed".into());
	*root = aidash_domain::registry::bindings::ResolvedDefinition::new(
		root.identity.clone(),
		root.definition.clone(),
	)
	.unwrap();
	changed.validate().unwrap();
	assert!(matches!(
		federation
			.delegate_pinned(
				task,
				&changed.agent.registry_node,
				&changed.agent.local(),
				&changed
			)
			.await,
		Err(Error::Conflict(_))
	));
	assert_eq!(transport.offers.lock().unwrap().len(), 2);
}
