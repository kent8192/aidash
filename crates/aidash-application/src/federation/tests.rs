use super::*;
use crate::ports::federation::PeerReply;
use aidash_domain::federation::Peer;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::BTreeMap, sync::Mutex};

struct Repository {
	peers: Vec<Peer>,
	pages: BTreeMap<u64, Value>,
}
#[async_trait]
impl FederationRepository for Repository {
	async fn peers(&self) -> Result<Vec<Peer>> {
		Ok(self.peers.clone())
	}
	async fn peer(&self, node: &str) -> Result<Peer> {
		Ok(peer(node))
	}
	async fn agents(&self, search: &Search, offset: u64) -> Result<AgentPage> {
		assert_eq!(search.kind.as_deref(), Some("agent"));
		Ok(serde_json::from_value(self.pages[&offset].clone())?)
	}
}
struct Transport {
	status: u16,
	transaction_pending: bool,
	run_message_pending: bool,
	pages: BTreeMap<(String, String), Value>,
	calls: Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl PeerTransport for Transport {
	async fn request(
		&self,
		peer: &Peer,
		_method: &str,
		path: &str,
		_body: Option<&Value>,
	) -> Result<PeerReply> {
		self.calls
			.lock()
			.unwrap()
			.push((peer.node_id.clone(), path.into()));
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
	Repository {
		peers: vec![],
		pages: BTreeMap::from([(0, json!({"entries":[],"next_offset":null}))]),
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
