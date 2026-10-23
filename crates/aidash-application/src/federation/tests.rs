use super::*;
use crate::ports::federation::PeerReply;
use aidash_domain::federation::route::Route;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::BTreeMap, sync::Mutex};

struct Repository {
	routes: Vec<Route>,
}
#[async_trait]
impl FederationRepository for Repository {
	async fn routes(&self) -> Result<Vec<Route>> {
		Ok(self.routes.clone())
	}
	async fn route(&self, node: &str) -> Result<Route> {
		Ok(peer(node))
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
		peer: &Route,
		_method: &str,
		path: &str,
		_body: Option<&Value>,
	) -> Result<PeerReply> {
		self.calls
			.lock()
			.unwrap()
			.push((peer.target.deployment.to_string(), path.into()));
		if peer.target.deployment.as_str() == "aidash://offline" {
			return Err(Error::External("peer unavailable".into()));
		}
		Ok(PeerReply {
			status: self.status,
			status_label: self.status.to_string(),
			transaction_pending: self.transaction_pending,
			run_message_pending: self.run_message_pending,
			body: self
				.pages
				.get(&(peer.target.deployment.to_string(), path.into()))
				.cloned()
				.unwrap_or(Value::Null),
		})
	}
}
fn peer(node: &str) -> Route {
	crate::test_support::foreign_route(node)
}
#[fixture]
fn repository() -> Repository {
	Repository { routes: vec![] }
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
	Federation::new(repository, transport)
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
async fn malformed_peer_success_is_an_external_failure(
	repository: Repository,
	transport: Transport,
) {
	let result = federation(Arc::new(repository), Arc::new(transport))
		.request::<aidash_domain::registry::Entry>("aidash://remote", "GET", "/task", None)
		.await;
	assert!(
		matches!(result,Err(Error::External(message)) if message.starts_with("invalid response JSON:"))
	);
}
