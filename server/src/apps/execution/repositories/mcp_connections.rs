//! Resolve MCP authentication only from the local Run's chain-root person.
use aidash_application::oauth_connections::{Owner, Service};
use aidash_application::ports::tools::{PluginInputs, PluginServer, PluginSessions};
use aidash_application::tenant_secrets::{
	BearerSource, CallSecret, ConnectionRecord, SecretCall, SecretRoute, SecretRouteSource,
};
use aidash_application::{Error, Result};
use aidash_domain::{
	Run,
	agent_plugin::{
		ListedTool, RemoteServer,
		interaction::{CallCheckpoint, ToolRound},
	},
};
use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

struct Source {
	id: Uuid,
	service: Arc<Service>,
	owner: Owner,
	resource: String,
}
#[async_trait]
impl BearerSource for Source {
	async fn bearer(&self) -> Result<SecretString> {
		let bearer = self.service.bearer(&self.owner, &self.resource).await?;
		if bearer.connection != self.id {
			// Disconnect + reconnect must not silently change the invocation's recorded owner grant.
			return Err(Error::Invalid(
				aidash_domain::oauth_connections::CONNECTION_REQUIRED.into(),
			));
		}
		Ok(bearer.access_token)
	}
	async fn invalidate(&self) {
		// `bearer` rechecks durable state and surfaces any refresh failure on retry.
		let _ = self.service.invalidate(&self.owner, &self.resource).await;
	}
}

struct BrokerSource {
	id: Uuid,
	service: Arc<Service>,
	owner: Owner,
	resource: String,
	call: SecretCall,
}
#[async_trait]
impl SecretRouteSource for BrokerSource {
	async fn route(&self) -> Result<SecretRoute> {
		self.service
			.brokered_route_for(&self.owner, &self.resource, self.id, self.call.clone())
			.await
	}
	async fn invalidate(&self) {
		let _ = self.service.invalidate(&self.owner, &self.resource).await;
	}
}

pub(crate) async fn connection(
	store: &crate::store::Store,
	run: &Run,
	scope: &aidash_application::tenancy::TenancyScope,
	resource: &str,
	call: SecretCall,
	key: &str,
) -> Result<CallSecret> {
	let required = || Error::Invalid(aidash_domain::oauth_connections::CONNECTION_REQUIRED.into());
	let service = store.oauth_connections.as_ref().ok_or_else(required)?;
	let owner = crate::apps::identity::repositories::oauth_connections::owner_of_run(store, run)
		.await?
		.ok_or_else(required)?;
	if scope
		.tenant_name()
		.is_none_or(|tenant| tenant.as_str() != owner.tenant)
	{
		return Err(Error::Forbidden);
	}
	let resource = aidash_domain::oauth_connections::canonical_resource(resource)?;
	let id = if service.calls.is_some() {
		service.brokered_connection(&owner, &resource).await?
	} else {
		service.bearer(&owner, &resource).await?.connection
	};
	let record = ConnectionRecord {
		id,
		host: reqwest::Url::parse(&resource)
			.map_err(|_| required())?
			.host_str()
			.ok_or_else(required)?
			.into(),
		resource: resource.clone(),
	};
	if !key.is_empty() {
		store.invocation_connection(scope, key, &record).await?;
	}
	if service.calls.is_some() {
		return Ok(CallSecret::BrokeredConnection {
			record,
			rest: aidash_application::tenant_secrets::connection_forward_rest(
				&resource,
				&call.destination,
			)?,
			source: Arc::new(BrokerSource {
				id,
				service: service.clone(),
				owner,
				resource,
				call,
			}),
		});
	}
	Ok(CallSecret::Connection {
		record,
		source: Arc::new(Source {
			id,
			service: service.clone(),
			owner,
			resource,
		}),
	})
}

/// The application uses one session port for listing and dispatch; this decorator binds both to
/// the same Run. Workbench keeps the undecorated port and has no person's credentials.
pub(crate) struct Sessions {
	pub store: crate::store::Store,
	pub run: Run,
	pub scope: aidash_application::tenancy::TenancyScope,
}
impl Sessions {
	async fn auth(
		&self,
		run: Uuid,
		config: &RemoteServer,
		key: &str,
	) -> Result<Option<CallSecret>> {
		if run != self.run.id {
			return Err(Error::Forbidden);
		}
		let resource = aidash_domain::oauth_connections::canonical_resource(config.url.as_str())?;
		let call = SecretCall {
			destination: aidash_application::tenant_secrets::endpoint_destination(
				config.url.as_str(),
			)?,
			methods: vec!["POST".into(), "GET".into(), "DELETE".into()],
			attach: aidash_capability::tenant_secret::Attach::Bearer,
			subject: aidash_capability::tenant_secret::SecretSubject::Run {
				run: self.run.id.to_string(),
				call: Uuid::new_v4(),
			},
			max_response_bytes: None,
		};
		match connection(&self.store, &self.run, &self.scope, &resource, call, key).await {
			Ok(auth) => Ok(Some(auth)),
			Err(Error::Invalid(code))
				if code == aidash_domain::oauth_connections::CONNECTION_REQUIRED =>
			{
				Ok(None)
			}
			Err(error) => Err(error),
		}
	}
}
#[async_trait]
impl PluginSessions for Sessions {
	async fn list(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		_auth: Option<&CallSecret>,
	) -> Result<Vec<ListedTool>> {
		let auth = self.auth(run, config, "").await?;
		self.store
			.plugin_sessions
			.list(run, server, config, auth.as_ref())
			.await
	}
	async fn call(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		tool: &str,
		schema_digest: &str,
		arguments: Value,
		key: &str,
		_auth: Option<&CallSecret>,
	) -> Result<Value> {
		let auth = self.auth(run, config, key).await?;
		self.store
			.plugin_sessions
			.call(
				run,
				server,
				config,
				tool,
				schema_digest,
				arguments,
				key,
				auth.as_ref(),
			)
			.await
	}
	async fn round(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		tool: &str,
		schema_digest: &str,
		arguments: Value,
		key: &str,
		checkpoint: Option<&CallCheckpoint>,
		inputs: Option<Arc<dyn PluginInputs>>,
		_auth: Option<&CallSecret>,
	) -> Result<ToolRound> {
		let auth = self.auth(run, config, key).await?;
		self.store
			.plugin_sessions
			.round(
				run,
				server,
				config,
				tool,
				schema_digest,
				arguments,
				key,
				checkpoint,
				inputs,
				auth.as_ref(),
			)
			.await
	}
	async fn close(&self, run: Uuid) {
		self.store.plugin_sessions.close(run).await;
	}
}
