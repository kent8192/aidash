use super::*;
use crate::ports::federation::peers::PeerWrite;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex};

struct Scope {
	peers: Mutex<Vec<Peer>>,
	secrets: BTreeMap<String, String>,
	trace: Arc<Mutex<Vec<String>>>,
	matching_identity: bool,
}
#[fixture]
fn peer() -> Peer {
	Peer {
		node_id: "aidash://remote".into(),
		endpoint: "https://peer.example.test".into(),
		credential_env: "AIDASH_SECRET_REMOTE".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	}
}
#[fixture]
fn scope(peer: Peer) -> Scope {
	Scope {
		peers: Mutex::new(vec![peer]),
		secrets: BTreeMap::from([("AIDASH_SECRET_REMOTE".into(), "current credential".into())]),
		trace: Arc::new(Mutex::new(vec![])),
		matching_identity: true,
	}
}
impl Credentials for Scope {
	fn resolve(&self, reference: &str) -> Result<String> {
		self.trace
			.lock()
			.unwrap()
			.push(format!("secret:{reference}"));
		self.secrets
			.get(reference)
			.cloned()
			.ok_or_else(|| Error::Invalid("credential is not configured".into()))
	}
}
#[async_trait]
impl PeerIdentity for Scope {
	async fn identity(&self, peer: &Peer) -> Result<Value> {
		self.trace.lock().unwrap().push("identity".into());
		Ok(
			json!({"id":if self.matching_identity {peer.node_id.as_str()} else {"aidash://different"},"protocol_version":"0.1"}),
		)
	}
}
struct Write {
	peer: Peer,
	trace: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl PeerWrite for Write {
	async fn disable(&mut self, _: &str) -> Result<Peer> {
		self.trace.lock().unwrap().push("disable".into());
		self.peer.enabled = false;
		Ok(self.peer.clone())
	}
	async fn register(&mut self, peer: Peer, _: &str) -> Result<Peer> {
		self.trace.lock().unwrap().push("atomic-register".into());
		Ok(peer)
	}
}
#[async_trait]
impl PeerConfiguration for Scope {
	async fn write_scope(&self) -> Result<Box<dyn PeerWrite>> {
		self.trace.lock().unwrap().push("scope".into());
		Ok(Box::new(Write {
			peer: self.peers.lock().unwrap()[0].clone(),
			trace: self.trace.clone(),
		}))
	}
	async fn enabled(&self, node: &str) -> Result<Peer> {
		self.peers
			.lock()
			.unwrap()
			.iter()
			.find(|peer| peer.node_id == node && peer.enabled)
			.cloned()
			.ok_or(Error::Unauthorized)
	}
	async fn all(&self) -> Result<Vec<Peer>> {
		Ok(self.peers.lock().unwrap().clone())
	}
	async fn authorized(&self, _: &str, _: Uuid, _: &EntityRef) -> Result<bool> {
		Ok(false)
	}
}
fn authority(scope: Arc<Scope>) -> PeerAuthority {
	PeerAuthority {
		node: "aidash://local".into(),
		protocol: "0.1".into(),
		configuration: scope.clone(),
		credentials: scope.clone(),
		identity: scope,
	}
}

#[rstest]
#[tokio::test]
async fn registration_checks_remote_identity_before_atomic_commit(scope: Scope, peer: Peer) {
	let scope = Arc::new(scope);
	let registered = authority(scope.clone()).register(peer).await.unwrap();
	assert_eq!(registered.node_id, "aidash://remote");
	assert_eq!(
		*scope.trace.lock().unwrap(),
		[
			"scope",
			"secret:AIDASH_SECRET_REMOTE",
			"identity",
			"atomic-register"
		]
	);
}

#[rstest]
#[tokio::test]
async fn disabling_a_peer_uses_its_stored_definition_without_external_credentials(
	scope: Scope,
	mut peer: Peer,
) {
	peer.enabled = false;
	peer.credential_env = "AIDASH_SECRET_MISSING".into();
	peer.endpoint = "https://changed.example.test".into();
	let scope = Arc::new(scope);
	let disabled = authority(scope.clone()).register(peer).await.unwrap();
	assert!(!disabled.enabled);
	assert_eq!(disabled.endpoint, "https://peer.example.test");
	assert_eq!(*scope.trace.lock().unwrap(), ["scope", "disable"]);
}

#[rstest]
#[tokio::test]
async fn mismatched_identity_never_commits_configuration(mut scope: Scope, peer: Peer) {
	scope.matching_identity = false;
	let scope = Arc::new(scope);
	assert!(
		matches!(authority(scope.clone()).register(peer).await, Err(Error::Invalid(reason)) if reason == "peer identity or protocol does not match")
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["scope", "secret:AIDASH_SECRET_REMOTE", "identity"]
	);
}

#[rstest]
#[tokio::test]
async fn current_credential_cannot_authenticate_two_enabled_node_identities(
	scope: Scope,
	mut peer: Peer,
) {
	peer.node_id = "aidash://other".into();
	scope.peers.lock().unwrap().push(peer);
	assert!(matches!(
		authority(Arc::new(scope))
			.authenticate("aidash://remote", "current credential")
			.await,
		Err(Error::Unauthorized)
	));
}

#[rstest]
#[tokio::test]
async fn rotated_or_incorrect_bearer_fails_before_other_credentials_are_read(scope: Scope) {
	let scope = Arc::new(scope);
	assert!(matches!(
		authority(scope.clone())
			.authenticate("aidash://remote", "preceding credential")
			.await,
		Err(Error::Unauthorized)
	));
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["secret:AIDASH_SECRET_REMOTE"]
	);
}

#[rstest]
fn missing_enabled_peer_credential_fails_registration_closed(scope: Scope, mut peer: Peer) {
	peer.node_id = "aidash://other".into();
	peer.credential_env = "AIDASH_SECRET_MISSING".into();
	assert!(
		matches!(require_distinct_credentials(vec![peer], "aidash://remote", "current credential", &scope), Err(Error::Invalid(reason)) if reason == "credential is not configured")
	);
}

#[rstest]
#[tokio::test]
async fn absent_delegation_cannot_authorize_task_execution(scope: Scope) {
	assert!(matches!(
		authority(Arc::new(scope))
			.authorize_task(
				"aidash://remote",
				Uuid::new_v4(),
				&EntityRef {
					id: "agent".into(),
					version: "1".into()
				}
			)
			.await,
		Err(Error::Unauthorized)
	));
}
