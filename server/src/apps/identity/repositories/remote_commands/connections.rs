//! Home-owned personal calls resolve only destinations pinned by the verified remote grant.
use aidash_application::{
	Error, Result,
	oauth_connections::Owner,
	ports::tools::{PluginServer, PluginSessions, ToolTransport},
	tenant_secrets::{CallSecret, SecretCall, endpoint_destination},
};
use aidash_domain::{
	agent_plugin::{McpServer, RemoteServer, interaction::ToolRound},
	identity::commands::connections::{ConnectionCall, Kind, ToolKey},
	oauth_connections::{ConnectionState, canonical_resource},
	registry::bindings::BindingSnapshot,
	tool::{SecretOwner, ToolConfig, providers::ToolDescriptor},
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

mod journal;

enum Target {
	Registry(ToolConfig),
	Plugin {
		server: PluginServer,
		config: RemoteServer,
		schema_digest: Option<String>,
	},
}
fn target(snapshot: &BindingSnapshot, call: &ConnectionCall, list: bool) -> Result<Target> {
	let target = match &call.tool_key {
		ToolKey::Registry {
			registry_deployment,
			id,
			version,
		} => {
			if list {
				return Err(Error::Forbidden);
			}
			let entry = &snapshot
				.bindings
				.iter()
				.find(|b| {
					b.definition.kind == "tool"
						&& b.identity.registry_deployment == *registry_deployment
						&& b.identity.id == *id
						&& b.identity.version == *version
						&& b.excluded_reason.is_none()
				})
				.ok_or(Error::Forbidden)?
				.definition;
			let config = match aidash_domain::tool::legacy_config(&entry.config)? {
				Some(config) => config,
				None => serde_json::from_value::<ToolDescriptor>(entry.config.clone())?
					.transport
					.ok_or(Error::Forbidden)?,
			};
			jsonschema::validator_for(&entry.schema)
				.map_err(|_| Error::Forbidden)?
				.validate(&call.arguments)
				.map_err(|_| Error::Invalid("invalid tool arguments".into()))?;
			match &config {
				ToolConfig::Mcp {
					endpoint,
					tool_name,
					secret,
					..
				} if canonical_resource(endpoint).ok().as_deref()
					== Some(call.resource.as_str())
					&& *tool_name == call.tool_name
					&& *secret == call.secret_name
					&& call.kind == Kind::Connection => {}
				// A non-OAuth MCP endpoint is pinned verbatim, like an HTTP tool's.
				ToolConfig::Mcp {
					endpoint,
					tool_name,
					secret,
					..
				} if *endpoint == call.resource
					&& canonical_resource(endpoint).is_err()
					&& *tool_name == call.tool_name
					&& *secret == call.secret_name
					&& secret.is_some()
					&& call.kind == Kind::PersonalSecret => {}
				ToolConfig::Http {
					endpoint, secret, ..
				} if *endpoint == call.resource
					&& call.tool_name.is_empty()
					&& *secret == call.secret_name
					&& secret.is_some()
					&& call.kind == Kind::PersonalSecret => {}
				_ => return Err(Error::Forbidden),
			}
			Target::Registry(config)
		}
		ToolKey::Plugin {
			plugin,
			digest,
			server,
			schema_digest,
		} => {
			if call.kind != Kind::Connection
				|| call.secret_name.is_some()
				|| (!list && schema_digest.is_none())
			{
				return Err(Error::Forbidden);
			}
			let agent = aidash_application::plugin_tools::agent_bindings(snapshot)?;
			let reference = agent
				.agent_plugins
				.iter()
				.find(|p| p.name == *plugin && p.digest == *digest)
				.ok_or(Error::Forbidden)?;
			let Some(McpServer::Remote(config)) = reference.servers.get(server) else {
				return Err(Error::Forbidden);
			};
			if canonical_resource(config.url.as_str())? != call.resource {
				return Err(Error::Forbidden);
			}
			Target::Plugin {
				server: PluginServer {
					plugin: plugin.clone(),
					digest: digest.clone(),
					server: server.clone(),
				},
				config: config.clone(),
				schema_digest: schema_digest.clone(),
			}
		}
	};
	Ok(target)
}
/// Validate the admitted identity and destination before current policy or any replay lookup.
pub(super) fn authorization<'a>(
	snapshot: &'a BindingSnapshot,
	call: &ConnectionCall,
	list: bool,
) -> Result<(&'a aidash_domain::registry::Entry, &'static str)> {
	target(snapshot, call, list)?;
	let (definition, action) = match &call.tool_key {
		ToolKey::Registry {
			registry_deployment,
			id,
			version,
		} => (
			snapshot
				.bindings
				.iter()
				.find(|binding| {
					binding.identity.registry_deployment == *registry_deployment
						&& binding.identity.id == *id
						&& binding.identity.version == *version
				})
				.map(|binding| &binding.definition),
			"tool.invoke",
		),
		ToolKey::Plugin { .. } => (
			snapshot
				.definitions
				.iter()
				.find(|definition| definition.identity == snapshot.agent)
				.map(|definition| &definition.definition),
			"agent.execute",
		),
	};
	Ok((definition.ok_or(Error::Forbidden)?, action))
}

fn endpoint(target: &Target) -> &str {
	match target {
		Target::Registry(ToolConfig::Mcp { endpoint, .. } | ToolConfig::Http { endpoint, .. }) => {
			endpoint
		}
		Target::Plugin { config, .. } => config.url.as_str(),
		_ => unreachable!("external target"),
	}
}
fn personal_only(target: &Target) -> bool {
	matches!(
		target,
		Target::Registry(
			ToolConfig::Http {
				secret_owner: SecretOwner::Personal,
				..
			} | ToolConfig::Mcp {
				secret_owner: SecretOwner::Personal,
				..
			}
		)
	)
}
pub(super) fn unowned(
	snapshot: &BindingSnapshot,
	call: &ConnectionCall,
	list: bool,
) -> Result<Value> {
	let target = target(snapshot, call, list)?;
	Ok(if personal_only(&target) {
		json!({"outcome":"result","result":{"error":"secret_unavailable","resource":call.resource}})
	} else {
		json!({"outcome":"not_personal"})
	})
}
async fn auth(
	store: &crate::store::Store,
	owner: &Owner,
	call: &ConnectionCall,
	target: &Target,
	admission: Uuid,
) -> Result<Option<CallSecret>> {
	if call.kind == Kind::Connection
		&& let Some(service) = &store.oauth_connections
		&& service
			.list(owner)
			.await?
			.iter()
			.any(|c| c.resource == call.resource && c.state == ConnectionState::Active)
	{
		return crate::apps::execution::repositories::mcp_connections::for_owner(
			store,
			owner,
			&call.resource,
			secret_call(target, admission)?,
		)
		.await
		.map_err(|error| match error {
			// W4 service errors carry a stable code before transport; preserve its provenance.
			Error::Invalid(ref code)
				if code == aidash_domain::oauth_connections::CONNECTION_REQUIRED =>
			{
				Error::Connection(
					aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
						resource: call.resource.clone(),
					},
				)
			}
			other => other,
		})
		.map(Some);
	}
	if let (Some(name), Some(service)) = (&call.secret_name, &store.tenant_secrets)
		&& let Some(row) = service
			.personal(&owner.tenant, &owner.subject, name)
			.await?
	{
		// A pending or host-mismatched personal row cannot fall through to another person's authority.
		return service
			.call_secret_row(&row, endpoint(target), secret_call(target, admission)?)
			.await
			.map(Some);
	}
	if personal_only(target) {
		return Err(Error::Invalid("secret_unavailable".into()));
	}
	Ok(None)
}
/// The broker grant for a Home-side call: the pinned destination, its transport's methods and
/// the remote admission as the Run subject.
fn secret_call(target: &Target, admission: Uuid) -> Result<SecretCall> {
	Ok(SecretCall {
		destination: endpoint_destination(endpoint(target))?,
		methods: if matches!(target, Target::Registry(ToolConfig::Http { .. })) {
			vec!["POST".into()]
		} else {
			vec!["POST".into(), "GET".into(), "DELETE".into()]
		},
		attach: aidash_capability::tenant_secret::Attach::Bearer,
		subject: aidash_capability::tenant_secret::SecretSubject::Run {
			run: admission.to_string(),
			call: Uuid::now_v7(),
		},
		max_response_bytes: Some(256_000),
	})
}
fn error_value(error: &Error, resource: &str) -> Value {
	if let Error::Invalid(message) = error
		&& matches!(
			message.as_str(),
			"connection_unavailable" | "secret_unavailable"
		) {
		return json!({"error":message,"resource":resource});
	}
	if matches!(error, Error::External(message) if message == "connection_unavailable") {
		return json!({"error":"connection_unavailable","resource":resource});
	}
	// Adapter errors may contain HTTP details. Do not return those over Federation.
	json!({"error":"mcp_connection_call_failed"})
}

pub(super) async fn execute(
	store: &crate::store::Store,
	snapshot: &BindingSnapshot,
	grant: Uuid,
	admission: Uuid,
	owner: &Owner,
	call: &ConnectionCall,
	list: bool,
) -> Result<Value> {
	let target = target(snapshot, call, list)?;
	let journal = if list {
		None
	} else {
		match journal::start(store, owner, grant, admission, call).await? {
			journal::Start::Replay(result) => {
				return Ok(json!({"outcome":"result","result":result}));
			}
			journal::Start::Send(id) => Some(id),
		}
	};
	let result = async {
		let Some(auth) = auth(store, owner, call, &target, admission).await? else {
			return Ok(None);
		};
		if let Some(id) = journal {
			journal::authentication(store, owner, id, &auth).await?;
		}
		match target {
			Target::Registry(config) => {
				crate::bootstrap::tool_transport(store)
					.invoke(
						&config,
						call.arguments.clone(),
						&call.idempotency_key,
						Some(&auth),
					)
					.await
			}
			Target::Plugin {
				server,
				config,
				schema_digest,
			} => {
				// A fresh session per call; it is always ended before returning across Nodes.
				let sessions = aidash_integrations::plugin_sessions::McpPluginSessions::new(
					store.keyless.clone(),
					store.tool_clients.clone(),
				);
				let session = Uuid::now_v7();
				let result = if list {
					sessions
						.list(session, &server, &config, Some(&auth))
						.await
						.and_then(|tools| serde_json::to_value(tools).map_err(Into::into))
				} else {
					let refusing = Arc::new(RefuseInput::default());
					let result = match sessions
						.round(
							session,
							&server,
							&config,
							&call.tool_name,
							schema_digest.as_deref().unwrap_or_default(),
							// The Home journal executes at most once. Any peer response is
							// definitive, including an unsafe 5xx; do not retry it here.
							false,
							call.arguments.clone(),
							&call.idempotency_key,
							None,
							Some(refusing.clone()),
							Some(&auth),
						)
						.await
					{
						Ok(ToolRound::Complete(value)) => Ok(value),
						Ok(ToolRound::InputRequired(_)) => {
							Ok(json!({"error":"connection_remote_refused"}))
						}
						Err(error) => Err(error),
					};
					if refusing.0.load(std::sync::atomic::Ordering::SeqCst) {
						Ok(json!({"error":"connection_remote_refused"}))
					} else {
						result
					}
				};
				sessions.close(session).await;
				result
			}
		}
		.map(Some)
	}
	.await;
	let (value, failed) = match result {
		Ok(Some(value)) => (value, false),
		Ok(None) => {
			if let Some(id) = journal {
				journal::abandon(store, owner, id).await?;
			}
			return Ok(json!({"outcome":"not_personal"}));
		}
		Err(Error::Connection(cause)) => {
			if let Some(id) = journal {
				journal::abandon(store, owner, id).await?;
			}
			return Ok(json!({"outcome":"connection","cause":cause}));
		}
		Err(error) => (error_value(&error, &call.resource), true),
	};
	if let Some(id) = journal {
		journal::finish(store, owner, id, &value, failed).await?;
	}
	Ok(if list && !failed {
		json!({"outcome":"tools","tools":value})
	} else {
		json!({"outcome":"result","result":value})
	})
}
#[derive(Default)]
struct RefuseInput(std::sync::atomic::AtomicBool);
#[async_trait::async_trait]
impl aidash_application::ports::tools::PluginInputs for RefuseInput {
	async fn request(
		&self,
		_: String,
		_: aidash_domain::agent_plugin::interaction::InputRequest,
	) -> Result<Value> {
		self.0.store(true, std::sync::atomic::Ordering::SeqCst);
		Err(Error::Invalid("connection_remote_refused".into()))
	}
}
