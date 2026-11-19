//! Current tool authority, plugin delegation, and builtin visibility are use cases.
use crate::{
	Error, Result,
	capabilities::approvals::{CALL_UNAPPROVED, INVOKE},
	ports::authorization::tools::AgentToolRepository,
};
use aidash_domain::{
	RunMetadata,
	exposure::{self, CapabilityIdentity},
	federation::tool_lists::{
		OwnedRemoteToolLists, ToolVerdict, UNAVAILABLE_TOOL, list_key, registry_operation,
	},
	provider::ToolCall,
	registry::{
		AgentConfig, EntityRef,
		bindings::{QualifiedRef, SKILL_ASSET_READ},
	},
	tool::{
		AuthorityRefusal, AuthorizationRequirement, CorePermission, RefusalCheck, ResourceTarget,
		ToolConfig, ToolContract, ToolIdentity, ToolReach,
	},
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Replace foreign authority under one retained lease before evaluating a tool flag.
pub async fn refresh(repository: &dyn AgentToolRepository) -> Result<()> {
	if !repository.is_remote() {
		return Ok(());
	}
	let mut scope = repository.lease().await?;
	if scope.transaction_active() {
		scope.suspend().await?;
	}
	if !scope.replace_remote_authority().await? {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// The model named a task the Run cannot address; it can clear this by naming another.
fn task_not_found(id: &str) -> Error {
	Error::Invalid(format!(
		"task {id} was not found in this Workspace; use a task_id that task_create returned"
	))
}

/// Foreign execution can address only its admitted Task and Workspace.
pub fn remote_identifier(run: &RunMetadata, kind: &str, id: &str) -> Result<String> {
	// `memory` is no longer refused: a remote Run's memory tools are served by its Home (ADR 0126).
	if kind == "run" && id != run.id.to_string() {
		return Err(Error::Forbidden);
	}
	if (kind == "task" && id != run.task_id.to_string())
		|| (kind == "workspace" && id != run.workspace_id.to_string())
	{
		return Err(Error::Forbidden);
	}
	Ok(format!("{}/{kind}s/{id}", run.home_node))
}

/// The `<id>@<version>` an approval names for the tool, when the tool is one the
/// Tenant's HTTP, MCP or Agent Plugin policy approves outside its contract.
fn approval_reference(contract: &ToolContract) -> Option<(String, String)> {
	match &contract.identity {
		ToolIdentity::Descriptor(reference) => {
			Some((reference.id.clone(), reference.version.clone()))
		}
		ToolIdentity::Registry(reference) => {
			Some((reference.id.clone(), reference.version.clone()))
		}
		ToolIdentity::AgentPlugin(tool) => {
			let digest = tool.digest.strip_prefix("sha256:").unwrap_or(&tool.digest);
			Some((
				format!("agent-plugin:{}/{}/{}", tool.plugin, tool.server, tool.tool),
				digest.chars().take(12).collect(),
			))
		}
		ToolIdentity::Builtin(_) => None,
	}
}

/// The Run's authority refused `call`: an Authority Pause that names the tool and the check.
fn refused(call: &ToolCall, check: RefusalCheck, action: Option<&str>) -> Error {
	Error::AuthorityRefused(Box::new(AuthorityRefusal {
		tool: call.name.clone(),
		check,
		action: action.map(str::to_owned),
		subject: None,
		call: None,
	}))
}

/// What the Federation tool lists decide for a remote Run's call (ADR 0139). Without lists
/// (the Home shares the receiver's Tenant) only Reach never refuses, which `authorize` did
/// already. A tool that is blocked or Reach never answers the same words as a tool that does
/// not exist; one that a side merely lacks stops the Run so that side's people can answer.
///
/// `member` is a Subnode member of a Run that a Remote Grant started: it answers to the lists of
/// that creating Run (ADR 0110), except for the coordination tools, which act only inside the
/// Subnode. Nobody can be asked on a member's behalf, so a tool a side merely lacks is refused.
fn federation_gate(
	call: &ToolCall,
	lists: Option<&OwnedRemoteToolLists>,
	contract: &ToolContract,
	operation: &str,
	member: bool,
) -> Result<()> {
	let Some(lists) = lists else {
		return Ok(());
	};
	let key = list_key(contract, operation);
	if member && aidash_domain::subnodes::COORDINATION_TOOLS.contains(&key.as_str()) {
		return Ok(());
	}
	match lists.lists().decide(contract.reach, &key) {
		ToolVerdict::Usable => Ok(()),
		ToolVerdict::Unavailable => Err(Error::Invalid(UNAVAILABLE_TOOL.into())),
		ToolVerdict::Ask(_) if member => Err(Error::Refused(
			"this tool is not available to Subnode members of this Run".into(),
		)),
		// The key the lists matched travels as the refusal's action, so the pause can say which
		// pattern its answer adds without resolving the tool again.
		ToolVerdict::Ask(lacking) => Err(refused(
			call,
			RefusalCheck::FederationTools {
				home: lacking.home,
				receiver: lacking.receiver,
			},
			Some(&key),
		)),
	}
}

/// Whether work a remote Run started with the builtin tool `tool`, a running command, a Live
/// preview or a Subnode member's call, must now stop: the tool is no longer usable under both
/// Nodes' lists (ADR 0139, decision 12). A Block added, an Allowance or a Request that no
/// longer names it, or a Reach that never answers all withdraw it, as a list change acts like a
/// withdrawal of authority. Without lists (the Home shares the receiver's Tenant) the Tenant's
/// policy decides and nothing is withdrawn here. A tool the table does not know withdraws too,
/// so a new tool must be classified before work depends on it.
pub fn tool_withdrawn(lists: Option<&OwnedRemoteToolLists>, tool: &str) -> bool {
	let Some(lists) = lists else {
		return false;
	};
	let Some(reach) = aidash_domain::tool::builtin_reach(tool) else {
		return true;
	};
	!matches!(lists.lists().decide(reach, tool), ToolVerdict::Usable)
}

/// Name the check behind a plain refusal (`Forbidden` or `Unauthorized`); every other outcome,
/// a refusal already named included, passes through.
fn named<T>(
	result: Result<T>,
	call: &ToolCall,
	check: RefusalCheck,
	action: Option<&str>,
) -> Result<T> {
	result.map_err(|error| match error {
		Error::Forbidden | Error::Unauthorized => refused(call, check, action),
		error => error,
	})
}

/// Resolve the pinned tool identity, never the model-chosen alias, against a Subnode allowance.
fn allowed_in_subnode(
	repository: &dyn AgentToolRepository,
	contract: &ToolContract,
	allowance: &aidash_domain::subnodes::Allowance,
) -> Result<bool> {
	let journal = matches!(&contract.identity, ToolIdentity::Builtin(name)
		if ["builtin:journal_query", "builtin:journal_search", "builtin:journal_read"].contains(&name.as_str()));
	let (name, secret) = match &contract.identity {
		ToolIdentity::Builtin(name) => (
			name.strip_prefix("builtin:").unwrap_or(name).to_owned(),
			None,
		),
		ToolIdentity::Registry(reference) => {
			let Some(snapshot) = repository.binding_snapshot() else {
				return Ok(false);
			};
			let mut matches = snapshot.bindings.iter().filter(|binding| {
				binding.identity.local() == *reference
					&& binding.definition.kind == "tool"
					&& binding.excluded_reason.is_none()
			});
			let Some(binding) = matches.next() else {
				return Ok(false);
			};
			if matches.next().is_some() {
				return Ok(false);
			}
			(
				binding.identity.resource_id(),
				aidash_domain::tenant_secrets::definition_secret(&binding.definition),
			)
		}
		ToolIdentity::Descriptor(reference) => {
			let Some(binding) = repository.binding_snapshot().and_then(|snapshot| {
				snapshot.bindings.iter().find(|binding| {
					binding.identity == *reference && binding.excluded_reason.is_none()
				})
			}) else {
				return Ok(false);
			};
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(binding.definition.config.clone())?;
			let name = if descriptor.provider.starts_with("core.") {
				descriptor.operation
			} else {
				reference.resource_id()
			};
			(
				name,
				aidash_domain::tenant_secrets::definition_secret(&binding.definition),
			)
		}
		ToolIdentity::AgentPlugin(_) => return Ok(false),
	};
	Ok((journal
		|| aidash_domain::subnodes::COORDINATION_TOOLS.contains(&name.as_str())
		|| allowance.tools.contains(&name))
		&& secret.is_none_or(|secret| allowance.secrets.contains(&secret)))
}

pub async fn authorize(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	configuration: &AgentConfig,
	call: &ToolCall,
	contract: &ToolContract,
) -> Result<()> {
	if repository.is_remote()
		&& matches!(&contract.identity, ToolIdentity::Builtin(grant)
			if matches!(grant.as_str(), "builtin:task_delegate" | "builtin:agent_discover"))
	{
		return Err(Error::Invalid(
			"a Run started by a Remote Grant is limited to one hop and cannot discover agents or delegate tasks".into(),
		));
	}
	named(
		refresh(repository).await,
		call,
		RefusalCheck::RemoteAuthority,
		None,
	)?;
	// Reach never needs no list; the answer is the one a blocked or unknown tool gets.
	if repository.is_remote() && contract.reach == ToolReach::Never {
		return Err(Error::Invalid(UNAVAILABLE_TOOL.into()));
	}
	if contract
		.authorization
		.flag
		.is_some_and(|flag| !flag.permitted(configuration))
	{
		return Err(refused(call, RefusalCheck::AgentFlag, None));
	}
	// Taking the lease only waits for the Access guard; it refuses nothing.
	let mut scope = repository.lease().await?;
	let allowance = scope.subnode_allowance().await?;
	if let Some(allowance) = &allowance
		&& !allowed_in_subnode(repository, contract, allowance)?
	{
		return Err(refused(call, RefusalCheck::Policy, None));
	}
	// The refresh above read the Home's current Request; the receiver's lists are its own rows.
	// A Subnode member is local, but answers to the lists of the Run that created its Subnode,
	// which the repository finds; it has none when that Run is not a remote one.
	let member = !repository.is_remote() && allowance.is_some();
	let lists = if repository.is_remote() || member {
		scope.remote_tool_lists().await?
	} else {
		None
	};
	let mut current;
	let mut operation = None;
	let authorization = match &contract.identity {
		ToolIdentity::Descriptor(reference) => {
			let entry = named(
				scope
					.catalog(&reference.local(), "tool.invoke", &call.name)
					.await,
				call,
				RefusalCheck::Catalog,
				Some("tool.invoke"),
			)?;
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config)?;
			current = descriptor.declared_contract(reference.clone())?;
			// Registry narrowing (or a Run pinned before the provider declared
			// it) may only lower Concurrency; every other attribute must match.
			current.behavior.concurrency = current
				.behavior
				.concurrency
				.narrowed(Some(contract.behavior.concurrency));
			if current != *contract {
				return Err(Error::Conflict("admitted provider contract changed".into()));
			}
			if repository.is_remote()
				&& descriptor.provider == "core.tasks@1"
				&& matches!(
					descriptor.operation.as_str(),
					"task_delegate" | "agent_discover"
				) {
				return Err(Error::Invalid(
					"a Run started by a Remote Grant is limited to one hop and cannot discover agents or delegate tasks".into(),
				));
			}
			federation_gate(
				call,
				lists.as_ref(),
				contract,
				&descriptor.operation,
				member,
			)?;
			operation = Some(descriptor.operation);
			&current.authorization
		}
		ToolIdentity::Registry(reference) => {
			// The model picks tools from the Run's own tool list, so a Registry tool the
			// configuration lacks means the agent's configuration changed: an authority change,
			// unlike a `skill` argument, which the model types.
			if !configuration.tools.contains(reference) {
				return Err(refused(call, RefusalCheck::RegistryTool, None));
			}
			let entry = named(
				scope.catalog(reference, "tool.invoke", &call.name).await,
				call,
				RefusalCheck::Catalog,
				Some("tool.invoke"),
			)?;
			let config: ToolConfig = serde_json::from_value(entry.config)?;
			federation_gate(
				call,
				lists.as_ref(),
				contract,
				registry_operation(&config),
				member,
			)?;
			current = ToolContract::registry(reference.clone(), &config);
			&current.authorization
		}
		ToolIdentity::Builtin(grant) => {
			federation_gate(call, lists.as_ref(), contract, "", member)?;
			let resource = scope.resource("tool", grant, json!({}));
			named(
				scope.require(&resource, "tool.invoke").await,
				call,
				RefusalCheck::Policy,
				Some("tool.invoke"),
			)?;
			operation = grant.strip_prefix("builtin:").map(str::to_owned);
			&contract.authorization
		}
		// The Agent version's Catalog Approval covers the servers of its plugin copies (ADR 0072),
		// so a plugin tool has no Registry entry. It needs the copy to be referenced and the Run
		// to have pinned exactly this server and tool.
		ToolIdentity::AgentPlugin(identity) => {
			let pinned = configuration
				.agent_plugins
				.iter()
				.any(|plugin| plugin.name == identity.plugin && plugin.digest == identity.digest)
				.then(|| {
					repository
						.plugin_tools()?
						.servers
						.iter()
						.filter(|server| {
							server.plugin == identity.plugin
								&& server.digest == identity.digest
								&& server.server == identity.server
						})
						.flat_map(|server| &server.tools)
						.find(|tool| tool.name == identity.tool)
				})
				.flatten()
				.ok_or_else(|| refused(call, RefusalCheck::PluginTool, None))?;
			// A stdio server's tools need no Tool call approval (ADR 0073).
			let stdio = configuration
				.agent_plugins
				.iter()
				.find(|plugin| plugin.name == identity.plugin && plugin.digest == identity.digest)
				.and_then(|plugin| plugin.servers.get(&identity.server))
				.is_some_and(|server| {
					matches!(server, aidash_domain::agent_plugin::McpServer::Stdio(_))
				});
			// The `mcp_stdio` approval and the shell's dispatch gates are checked by the
			// session host at every call, outside this Run-lease Catalog scope, whose
			// approvals hold only the Run's Binding closure.
			current = if stdio {
				ToolContract::agent_plugin_stdio(identity.clone())
			} else {
				ToolContract::agent_plugin(identity.clone(), pinned.read_only)
			};
			if current != *contract {
				return Err(Error::Conflict(
					"admitted plugin tool contract changed".into(),
				));
			}
			&current.authorization
		}
	};
	let configured_skill = authorization
		.requirements
		.contains(&AuthorizationRequirement::ConfiguredSkill);
	// Skill asset reads name a Discoverable capability alias; authority follows
	// the identity kind behind it.
	let asset_read = configured_skill && operation.as_deref() == Some(SKILL_ASSET_READ);
	let legacy_skill = configured_skill && !asset_read && call.arguments.get("skill_id").is_none();
	if !legacy_skill
		&& !asset_read
		&& authorization
			.core
			.is_some_and(|permission| !permission.permitted(&configuration.core_capabilities))
	{
		return Err(refused(call, RefusalCheck::CoreCapability, None));
	}
	for requirement in &authorization.requirements {
		match requirement {
			AuthorizationRequirement::AgentExecution { node_id, agent } => {
				// The Node comes from the agent's configuration, not from the model.
				if configuration.allow_task_delegation == Some(false)
					|| !aidash_domain::nodes::names_home(node_id, &run.home_node)
				{
					return Err(refused(call, RefusalCheck::AgentExecution, None));
				}
				named(
					scope.catalog(agent, "agent.execute", &call.name).await,
					call,
					RefusalCheck::Catalog,
					Some("agent.execute"),
				)?;
				let resource =
					scope.resource("workspace", &run.workspace_id.to_string(), json!({}));
				named(
					scope.require(&resource, "task.create").await,
					call,
					RefusalCheck::Policy,
					Some("task.create"),
				)?;
			}
			AuthorizationRequirement::ConfiguredSkill => {
				if asset_read {
					match asset_skill(repository, call)? {
						Some(reference) => {
							let reference = reference.local();
							if !configuration.skills.contains(&reference) {
								return Err(unconfigured_skill(&reference));
							}
							let entry = named(
								scope.catalog(&reference, "skill.use", &call.name).await,
								call,
								RefusalCheck::Catalog,
								Some("skill.use"),
							)?;
							if entry.kind != "skill" {
								return Err(not_a_skill(&reference));
							}
						}
						None => {
							// Direct Skills require the core Skills capability. Without
							// direct Skill sources the alias names no asset, and the
							// tool returns its recoverable UNKNOWN_CAPABILITY.
							let direct = !configuration.skill_attachments.is_empty()
								|| !configuration.skill_roots.is_empty();
							if direct
								&& !CorePermission::Skills
									.permitted(&configuration.core_capabilities)
							{
								return Err(refused(call, RefusalCheck::CoreCapability, None));
							}
						}
					}
					continue;
				}
				if !legacy_skill {
					continue;
				}
				let reference: EntityRef = serde_json::from_value(call.arguments["skill"].clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				if !configuration.skills.contains(&reference) {
					return Err(unconfigured_skill(&reference));
				}
				let entry = named(
					scope.catalog(&reference, "skill.use", &call.name).await,
					call,
					RefusalCheck::Catalog,
					Some("skill.use"),
				)?;
				if entry.kind != "skill" {
					return Err(not_a_skill(&reference));
				}
			}
			AuthorizationRequirement::LocalDelegation => {
				call.arguments["task_id"]
					.as_str()
					.and_then(|s| s.parse::<Uuid>().ok())
					.ok_or_else(|| Error::Invalid("invalid task id".into()))?;
				let node = call.arguments["node_id"]
					.as_str()
					.ok_or_else(|| Error::Invalid("missing node_id".into()))?;
				let reference: EntityRef = serde_json::from_value(call.arguments["agent"].clone())?;
				if aidash_domain::nodes::names_home(node, &run.home_node) {
					named(
						scope.catalog(&reference, "agent.execute", &call.name).await,
						call,
						RefusalCheck::Catalog,
						Some("agent.execute"),
					)?;
				} else {
					aidash_domain::nodes::NodeAddress::parse(node)?;
					let resource = scope.resource("node", node, json!({"remote_node":node}));
					let denied = scope
						.explicitly_denied(&resource, "federation.execute")
						.await?;
					let decision = scope.require(&resource, "federation.execute").await;
					if denied && matches!(decision, Err(Error::Forbidden)) {
						return Err(Error::Invalid(
							"policy denies federation.execute on the target Node".into(),
						));
					}
					named(
						decision,
						call,
						RefusalCheck::Policy,
						Some("federation.execute"),
					)?;
				}
			}
			AuthorizationRequirement::Resource {
				action,
				kind,
				target,
			} => {
				let mut named_task = false;
				let id = match target {
					ResourceTarget::Workspace => run.workspace_id.to_string(),
					ResourceTarget::Task => run.task_id.to_string(),
					ResourceTarget::Agent => run.agent_id.clone(),
					ResourceTarget::Run => run.id.to_string(),
					ResourceTarget::ArgumentUuid(field) => {
						named_task = *kind == "task";
						call.arguments[field]
							.as_str()
							.and_then(|s| s.parse::<Uuid>().ok())
							.ok_or_else(|| Error::Invalid("invalid task id".into()))?
							.to_string()
					}
					ResourceTarget::Argument(field) => call.arguments[field]
						.as_str()
						.ok_or_else(|| Error::Invalid("missing generation policy".into()))?
						.to_owned(),
				};
				let resource = protected_resource(
					scope.as_mut(),
					repository.is_remote(),
					run,
					kind,
					&id,
					named_task,
				)
				.await;
				let resource = named(
					resource,
					call,
					if repository.is_remote() {
						RefusalCheck::RemoteAuthority
					} else {
						RefusalCheck::Policy
					},
					Some(action),
				)?;
				named(
					scope.require(&resource, action).await,
					call,
					RefusalCheck::Policy,
					Some(action),
				)?;
			}
		}
	}
	// A Read-only operation needs no approval, but the explicit deny of
	// `tool.call_unapproved` still refuses it. The capability approval path
	// checks this on the Run's home Node; doing it with the contract authority
	// also covers a Run executing for another Node.
	// A stdio plugin tool needs no approval either (ADR 0073), so the same deny refuses it.
	let unapproved_plugin_call = matches!(contract.identity, ToolIdentity::AgentPlugin(_))
		&& !contract.behavior.tool_call_approval;
	if (contract.behavior.read_only_operation || unapproved_plugin_call)
		&& let Some((id, version)) = approval_reference(contract)
	{
		let resource = scope.resource(
			"tool",
			&id,
			json!({
				"version": version,
				"operation": INVOKE,
				"run_id": run.id,
				"action": CALL_UNAPPROVED,
			}),
		);
		let denied = named(
			scope.explicitly_denied(&resource, CALL_UNAPPROVED).await,
			call,
			RefusalCheck::CallUnapprovedDeny,
			Some(CALL_UNAPPROVED),
		)?;
		if denied {
			return Err(refused(
				call,
				RefusalCheck::CallUnapprovedDeny,
				Some(CALL_UNAPPROVED),
			));
		}
	}
	Ok(())
}

/// A Skill the agent is not configured with is the model's mistake, not a change of authority.
fn unconfigured_skill(reference: &EntityRef) -> Error {
	Error::Invalid(format!(
		"skill {} is not configured for this agent; use a skill listed for this agent",
		reference_label(reference),
	))
}

fn not_a_skill(reference: &EntityRef) -> Error {
	Error::Invalid(format!(
		"{} is not a skill; use a skill listed for this agent",
		reference_label(reference),
	))
}

fn reference_label(reference: &EntityRef) -> String {
	format!("{}@{}", reference.id, reference.version)
}

/// The Registry Skill a `skill_asset_read` alias names in the Run's snapshot;
/// `None` means any other alias: a direct Skill or nothing.
fn asset_skill(
	repository: &dyn AgentToolRepository,
	call: &ToolCall,
) -> Result<Option<QualifiedRef>> {
	let alias = call.arguments["alias"]
		.as_str()
		.ok_or_else(|| Error::Invalid("alias must be a string".into()))?;
	let snapshot = repository
		.binding_snapshot()
		.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
	Ok(exposure::catalog(snapshot, &BTreeMap::new(), &[], None)?
		.into_iter()
		.find(|capability| capability.alias == alias)
		.and_then(|capability| match capability.identity {
			CapabilityIdentity::Registry(reference) => Some(reference),
			CapabilityIdentity::DirectSkill { .. } | CapabilityIdentity::AgentPluginTool { .. } => {
				None
			}
		}))
}

/// Filter the assembled tools against the current Subnode allowance, if any.
pub async fn filter_subnode<T: Send>(
	repository: &dyn AgentToolRepository,
	tools: &mut BTreeMap<String, T>,
	contract: impl Fn(&T) -> ToolContract,
) -> Result<bool> {
	let mut scope = repository.lease().await?;
	let Some(allowance) = scope.subnode_allowance().await? else {
		return Ok(false);
	};
	for name in tools.keys().cloned().collect::<Vec<_>>() {
		if !allowed_in_subnode(repository, &contract(&tools[&name]), &allowance)? {
			tools.remove(&name);
		}
	}
	Ok(true)
}

/// The operation `list_key` needs for a tool, read from the Run's pinned Binding snapshot the
/// way `authorize` reads it from the Catalog entry; `None` when the snapshot does not name it.
fn pinned_operation(
	repository: &dyn AgentToolRepository,
	contract: &ToolContract,
) -> Option<String> {
	let snapshot = repository.binding_snapshot();
	match &contract.identity {
		ToolIdentity::Builtin(_) => Some(String::new()),
		ToolIdentity::Registry(reference) => {
			let binding = snapshot?.bindings.iter().find(|binding| {
				binding.identity.local() == *reference
					&& binding.definition.kind == "tool"
					&& binding.excluded_reason.is_none()
			})?;
			let config: ToolConfig =
				serde_json::from_value(binding.definition.config.clone()).ok()?;
			Some(registry_operation(&config).to_owned())
		}
		ToolIdentity::Descriptor(reference) => {
			let binding = snapshot?.bindings.iter().find(|binding| {
				binding.identity == *reference && binding.excluded_reason.is_none()
			})?;
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(binding.definition.config.clone()).ok()?;
			Some(descriptor.operation)
		}
		ToolIdentity::AgentPlugin(_) => None,
	}
}

/// Take from a remote Run's tool map what it may never call: tools of Reach never, and tools the
/// receiving Node blocks for the Home. Every other candidate stays, even one a side has yet to
/// list, since calling it asks that side. A Block names only listable tools: a tool of Reach Run
/// stays whatever the Block says, as `RemoteToolLists::decide` lets it run (ADR 0139).
pub async fn filter_remote<T: Send>(
	repository: &dyn AgentToolRepository,
	tools: &mut BTreeMap<String, T>,
	contract: impl Fn(&T) -> ToolContract,
) -> Result<()> {
	if !repository.is_remote() {
		return Ok(());
	}
	let block = repository.lease().await?.remote_tool_block().await?;
	for name in tools.keys().cloned().collect::<Vec<_>>() {
		let declaration = contract(&tools[&name]);
		let blocked = declaration.reach.listable()
			&& block.as_ref().is_some_and(|block| {
				pinned_operation(repository, &declaration)
					.is_some_and(|operation| block.matches(&list_key(&declaration, &operation)))
			});
		if declaration.reach == ToolReach::Never || blocked {
			tools.remove(&name);
		}
	}
	Ok(())
}

/// Apply removals immediately so a later adapter failure retains earlier filtering.
pub async fn filter<T: Send>(
	repository: &dyn AgentToolRepository,
	agent: &AgentConfig,
	tools: &mut BTreeMap<String, T>,
	contract: impl Fn(&T) -> ToolContract,
) -> Result<()> {
	let mut scope = repository.lease().await?;
	if let Some(allowance) = scope.subnode_allowance().await? {
		for name in tools.keys().cloned().collect::<Vec<_>>() {
			if !allowed_in_subnode(repository, &contract(&tools[&name]), &allowance)? {
				tools.remove(&name);
			}
		}
	}
	for name in tools.keys().cloned().collect::<Vec<_>>() {
		let declaration = contract(&tools[&name]);
		if !declaration
			.authorization
			.core
			.is_some_and(|permission| permission.permitted(&agent.core_capabilities))
		{
			continue;
		}
		let ToolIdentity::Builtin(grant) = declaration.identity else {
			continue;
		};
		let resource = scope.resource("tool", &grant, json!({}));
		match scope.require(&resource, "tool.invoke").await {
			Ok(()) => {}
			Err(Error::Forbidden | Error::NotFound(_)) => {
				tools.remove(&name);
			}
			Err(error) => return Err(error),
		}
	}
	Ok(())
}

async fn protected_resource(
	scope: &mut dyn crate::ports::authorization::tools::AgentToolScope,
	remote: bool,
	run: &RunMetadata,
	kind: &str,
	id: &str,
	named_task: bool,
) -> Result<aidash_domain::policy::Resource> {
	Ok(if remote {
		// A foreign Run's admitted Task is the only one it can name; another is the model's
		// mistake, like a task the local Workspace does not hold.
		if named_task && kind == "task" && id != run.task_id.to_string() {
			return Err(task_not_found(id));
		}
		scope.resource(
			kind,
			&remote_identifier(run, kind, id)?,
			scope.context().clone(),
		)
	} else {
		match kind {
			"task" => {
				let task = scope
					.task_read(id.parse().map_err(|_| Error::Forbidden)?)
					.await;
				// The model named this task. One it cannot read looks like one that
				// does not exist, the same answer for both.
				let task = if named_task {
					task.map_err(|error| match error {
						Error::Forbidden | Error::NotFound(_) => task_not_found(id),
						error => error,
					})?
				} else {
					task?
				};
				scope.task_resource(&task).await?
			}
			"artifact" => {
				let creator = scope.subjects().last().ok_or(Error::Forbidden)?.clone();
				scope
					.artifact_creation_resource(run.task_id, &creator)
					.await?
			}
			"memory" => scope.memory_resource(run).await?,
			_ => scope.resource(kind, id, json!({})),
		}
	})
}

/// Apply the same resource authority to worker effects and explicit control-plane actions.
pub async fn action(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	action: &str,
	kind: &str,
	id: &str,
) -> Result<()> {
	refresh(repository).await?;
	let mut scope = repository.lease().await?;
	let resource =
		protected_resource(scope.as_mut(), repository.is_remote(), run, kind, id, false).await?;
	scope.require(&resource, action).await
}

/// A worker may disclose only a saved request for its own Run and Workspace.
pub async fn human_read(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	id: Uuid,
) -> Result<()> {
	if repository.is_remote() {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.lease().await?;
	let request = scope
		.human_request(run, id)
		.await?
		.ok_or(Error::Forbidden)?;
	let resource = scope.human_resource(&request).await?;
	scope.require(&resource, "human.read").await
}

#[cfg(test)]
mod tests;
