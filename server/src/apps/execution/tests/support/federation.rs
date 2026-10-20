//! Deployment Keys, Node Registrations and Federation tokens for fixtures (#241).
//! Every fixture Deployment that federates holds a key written the way
//! `manage deployment-key generate` writes it, and registers the Nodes it
//! accepts through the registration route, never through a table.
#![allow(dead_code)] // Each integration binary uses a different part of this module.
use aidash_application::federation::key::{DeploymentKey, Signing};
use aidash_capability::{
	TOKEN_TTL_SECS,
	federation::{FederationClaims, body_digest},
};
use aidash_domain::{federation::protocol::CURRENT, nodes::DeploymentKeyId};
use aidash_integrations::federation::{Egress, key::FileDeploymentKey};
use aidash_server::federation::Federation;
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::{
	collections::HashMap,
	path::PathBuf,
	sync::{Arc, LazyLock, Mutex, Weak},
};
use tempfile::TempDir;
use tokio::{io::AsyncReadExt, io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

/// A Deployment Key file in a temporary directory that lives as long as the
/// fixtures that hold the key.
pub struct KeyFile {
	_directory: TempDir,
	pub path: PathBuf,
	key: FileDeploymentKey,
}

impl KeyFile {
	pub fn key_id(&self) -> String {
		self.key.key_id().to_string()
	}

	pub fn public_key_pem(&self) -> &str {
		self.key.public_key_pem()
	}

	pub fn generate() -> Arc<Self> {
		let directory = tempfile::tempdir().expect("a directory for the Deployment Key");
		let path = directory.path().join("deployment-key.pem");
		let key = FileDeploymentKey::generate(&path, false).expect("a new Deployment Key");
		Arc::new(Self {
			_directory: directory,
			path,
			key,
		})
	}
}

/// The key a fixture Federation signs with. It owns its file so a worker
/// process started from the same fixture can load the same key.
struct FixtureKey(Arc<KeyFile>);

#[async_trait]
impl DeploymentKey for FixtureKey {
	fn key_id(&self) -> &DeploymentKeyId {
		self.0.key.key_id()
	}
	fn public_key_pem(&self) -> &str {
		self.0.key.public_key_pem()
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], aidash_capability::SignError> {
		self.0.key.sign(input).await
	}
}

static FILES: LazyLock<Mutex<HashMap<String, Weak<KeyFile>>>> = LazyLock::new(Default::default);

/// Gives `f` a new Deployment Key and turns Federation on, as `[deployment.key]
/// file` does at startup. Do this before the application is built from `f`.
pub fn give_key(f: &mut Federation) -> Arc<KeyFile> {
	let file = KeyFile::generate();
	let key: Arc<dyn DeploymentKey> = Arc::new(FixtureKey(file.clone()));
	FILES
		.lock()
		.unwrap()
		.insert(key.key_id().to_string(), Arc::downgrade(&file));
	f.config.federation = true;
	f.config.deployment_key = Some(key.clone());
	f.store.egress = Egress {
		enabled: true,
		key: Some(key),
	};
	file
}

/// [`give_key`] for a fixture that already serves: the keyed Federation
/// replaces the one the routes inject. Returns it, because the fixture's own
/// copy does not hold the key.
pub fn enable(f: &Federation, context: &reinhardt::InjectionContext) -> Federation {
	let mut keyed = f.clone();
	give_key(&mut keyed);
	context.set_singleton(keyed.clone());
	keyed
}

/// The file behind `f`'s Deployment Key, for a process started from the fixture.
pub fn key_file(f: &Federation) -> Option<PathBuf> {
	let id = f.config.deployment_key.as_ref()?.key_id().to_string();
	FILES
		.lock()
		.unwrap()
		.get(&id)?
		.upgrade()
		.map(|file| file.path.clone())
}

/// Points a process's `[deployment.key] file` at `f`'s key and keeps its switch.
pub fn configure_process(
	settings: &mut aidash_server::config::settings::ProjectSettings,
	f: &Federation,
) {
	settings.deployment.federation = f.config.federation;
	settings.deployment.key = key_file(f).map(|path| {
		serde_json::from_value(json!({ "file": path })).expect("a [deployment.key] file")
	});
}

pub fn key_id(f: &Federation) -> String {
	f.config
		.deployment_key
		.as_ref()
		.expect("the fixture Deployment holds a key")
		.key_id()
		.to_string()
}

/// Lets `f` reach the loopback origin of `endpoint`, where another fixture
/// Deployment (or its stand-in) answers.
pub fn permit(f: &Federation, endpoint: &str) {
	f.store
		.keyless
		.permit_origin_of(endpoint)
		.expect("a fixture origin");
}

pub fn registration_path(tenant: &str, node: &str) -> String {
	format!("/api/tenants/{tenant}/nodes/{node}/registrations")
}

/// The request that registers `source_node`. A Node of another Deployment names
/// its endpoint and key ID, which the server checks against what that
/// Deployment publishes.
pub fn registration(source_node: &str, other: Option<(&str, &str)>) -> Value {
	json!({
		"source_node": source_node,
		"endpoint": other.map(|(endpoint, _)| endpoint),
		"key_id": other.map(|(_, key_id)| key_id),
		"atomic_transactions": true, "enabled": true, "expected_revision": 0,
	})
}

/// Mints a Federation token for one request, as the sending Deployment's
/// transport does right before it sends. `path` is the request target under
/// `/federation/v0.1`, with its query.
pub async fn token(
	key: &dyn DeploymentKey,
	deployment: &str,
	sender: &str,
	receiver: &str,
	subject: &str,
	request: (&str, &str, &[u8]),
) -> String {
	let (method, path, body) = request;
	let now = chrono::Utc::now().timestamp().max(0) as u64;
	let claims = FederationClaims {
		iss: deployment.into(),
		sub: sender.into(),
		aud: receiver.into(),
		subject: subject.into(),
		iat: now,
		exp: now + TOKEN_TTL_SECS,
		jti: uuid::Uuid::new_v4(),
		kid: key.key_id().to_string(),
		protocol: CURRENT.into(),
		method: method.into(),
		path: path.into(),
		body: body_digest(body),
	};
	claims
		.mint(&Signing(key))
		.await
		.expect("the Federation token is minted")
		.expose_secret()
		.to_owned()
}

/// The headers a peer sends: the token and the protocol version.
pub fn peer_headers(token: &str) -> [(&'static str, String); 2] {
	[
		("authorization", format!("Bearer {token}")),
		("x-aidash-protocol", CURRENT.into()),
	]
}

/// A POST that `sender`'s default Node of the Tenant sends to `receiver` at
/// `endpoint`, signed with `sender`'s Deployment Key as `subject`. `path` is
/// the route under `/federation/v0.1`.
pub async fn signed(
	client: &reqwest::Client,
	endpoint: &str,
	path: &str,
	sender: (&Federation, &aidash_domain::nodes::NodeAddress),
	receiver: &aidash_domain::nodes::NodeAddress,
	subject: &str,
	body: &Value,
) -> reqwest::RequestBuilder {
	let (f, node) = sender;
	let bytes = body.to_string();
	let token = token(
		f.config
			.deployment_key
			.as_deref()
			.expect("the fixture Deployment holds a key"),
		f.config.deployment.as_str(),
		&node.to_string(),
		&receiver.to_string(),
		subject,
		("POST", path, bytes.as_bytes()),
	)
	.await;
	let mut request = client
		.post(format!("{endpoint}/federation/v0.1{path}"))
		.header("content-type", "application/json")
		.body(bytes);
	for (name, value) in peer_headers(&token) {
		request = request.header(name, value);
	}
	request
}

/// A Deployment that exists only as a name, a key and the document it
/// publishes at `/.well-known/aidash`. Tests register its Nodes and send
/// requests as them.
pub struct Remote {
	pub id: String,
	pub endpoint: String,
	pub key: Arc<KeyFile>,
	task: JoinHandle<()>,
}

impl Remote {
	pub async fn start(id: &str) -> Self {
		let key = KeyFile::generate();
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let identity = json!({
			"id": id, "endpoint": endpoint, "protocol_version": CURRENT,
			"key": {"id": key.key.key_id().as_str(), "public_key_pem": key.key.public_key_pem()},
		})
		.to_string();
		let task = tokio::spawn(async move {
			loop {
				let Ok((mut stream, _)) = listener.accept().await else {
					return;
				};
				let identity = identity.clone();
				tokio::spawn(async move {
					let mut request = vec![0u8; 4096];
					let _ = stream.read(&mut request).await;
					let response = format!(
						"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{identity}",
						identity.len()
					);
					let _ = stream.write_all(response.as_bytes()).await;
					let _ = stream.shutdown().await;
				});
			}
		});
		Self {
			id: id.into(),
			endpoint,
			key,
			task,
		}
	}

	pub fn key_id(&self) -> String {
		self.key.key.key_id().to_string()
	}

	pub fn node(&self, name: &str) -> String {
		format!("{}/{name}", self.id)
	}

	/// The registration body of one of this Deployment's Nodes.
	pub fn registration(&self, name: &str) -> Value {
		registration(&self.node(name), Some((&self.endpoint, &self.key_id())))
	}

	/// The headers of a request from `sender` (a Node name of this Deployment)
	/// to the Node `receiver`, as `subject`.
	pub async fn headers(
		&self,
		sender: &str,
		receiver: &str,
		subject: &str,
		request: (&str, &str, &[u8]),
	) -> [(&'static str, String); 2] {
		peer_headers(
			&token(
				&self.key.key,
				&self.id,
				&self.node(sender),
				receiver,
				subject,
				request,
			)
			.await,
		)
	}
}

impl Drop for Remote {
	fn drop(&mut self) {
		self.task.abort();
	}
}
