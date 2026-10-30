//! Node declarations are immutable catalog documents, independent of tenant grants.
use crate::{Result, ports::registry::DefinitionWriter, registry::DefinitionValidation};
use aidash_domain::{
	decision::{
		AnswerType, DECISIONS, DeciderConfig, DeciderRoute, Hook, Mode, Probability, StateBudget,
		policy::StateRetention,
	},
	registry::{
		Entry,
		bindings::{DEFAULT_TOOLS, EXPOSURE_TOOLS, REQUIRED_TOOLS, SKILL_ASSET_READ},
	},
	tool::providers::core_descriptor,
};
use serde_json::json;

/// The Decider that judges pending Tool calls in the Auto Approval Mode (ADR 0077).
pub use aidash_domain::registry::bindings::{COMPACTION_DECIDER_ID, COMPACTION_DECIDER_VERSION};

pub const APPROVAL_DECIDER_ID: &str = "aidash.approval-decider";
pub const APPROVAL_DECIDER_VERSION: &str = "1.0.0";

/// Every system builtin operation of either Exposure policy, in seeding order.
pub fn operations() -> impl Iterator<Item = &'static str> {
	REQUIRED_TOOLS
		.iter()
		.chain(DEFAULT_TOOLS)
		.chain(EXPOSURE_TOOLS)
		.copied()
		.chain([SKILL_ASSET_READ, TASK_ASSIGN])
}

/// Starts a generation inside Aidash only, so it is an ordinary built-in that
/// needs no Catalog Approval.
const TASK_ASSIGN: &str = "task_assign";

pub fn entries(validation: &DefinitionValidation, node: &str) -> Result<Vec<Entry>> {
	let specifications = validation.node_specifications();
	operations()
		.map(|operation| {
			let spec = specifications.get(operation).ok_or_else(|| {
				crate::Error::Invalid(format!(
					"system provider has no declaration for {operation}"
				))
			})?;
			Ok(Entry {
				binding_normalization: None,
				installation: None,
				id: format!("aidash.{operation}"),
				version: aidash_domain::registry::bindings::QualifiedRef::builtin(node, operation)
					.version,
				kind: "tool".into(),
				name: std::collections::BTreeMap::from([("en".into(), operation.to_string())]),
				description: std::collections::BTreeMap::from([(
					"en".into(),
					spec.description.clone(),
				)]),
				capabilities: vec![operation.to_string()],
				tags: vec!["system".into()],
				languages: vec![],
				skills: vec![],
				schema: spec.parameters.clone(),
				config: serde_json::to_value(core_descriptor(node, operation)?)?,
			})
		})
		.collect()
}

/// Keep the original immutable declarations available to admitted Runs and
/// existing Tenants when the workspace tools gain attachment support.
pub fn legacy_workspace_entries(
	validation: &DefinitionValidation,
	node: &str,
) -> Result<Vec<Entry>> {
	let mut entries: Vec<_> = entries(validation, node)?
		.into_iter()
		.filter(|entry| {
			matches!(
				entry.id.as_str(),
				"aidash.workspace_read" | "aidash.workspace_observe" | "aidash.workspace_message"
			)
		})
		.collect();
	for entry in &mut entries {
		entry.version = "1.0.0".into();
		let (description, schema) = match entry.id.as_str() {
			"aidash.workspace_message" => (
				"Send a message to the human and other agents sharing this workspace.",
				json!({"type":"object","required":["content"],"properties":{"content":{"type":"string"}},"additionalProperties":false}),
			),
			"aidash.workspace_observe" => (
				"Read a bounded summary of goal, tasks, artifact references, messages and recent event metadata. Use workspace_read for full records. Collections have separate totals and next_offset; events/messages are newest first. Refresh pagination if the workspace changes.",
				json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}),
			),
			"aidash.workspace_read" => (
				"Read one accessible workspace record by kind and exact ID. Returns a JSON text chunk, total_chars and next_offset; concatenate chunks in offset order to recover the full record. The returned chunk is capped to fit the active request budget. If budget_limited is true, continue from next_offset on a later turn; if deferred is true, stop reading until that later turn.",
				json!({"type":"object","required":["kind","id"],"properties":{"kind":{"enum":["workspace","task","artifact","message","event"]},"id":{"type":"string"},"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":0,"maximum":16000}},"additionalProperties":false}),
			),
			_ => unreachable!(),
		};
		entry.description.insert("en".into(), description.into());
		entry.schema = schema;
	}
	Ok(entries)
}

/// Every definition the Node declares to every Tenant's Registry: the built-in
/// tools and the approval and compaction Deciders. Unlike a tool, the Decider needs a Tenant's
/// Catalog Approval before anything may use it.
pub fn declarations(validation: &DefinitionValidation, node: &str) -> Result<Vec<Entry>> {
	let mut declared = entries(validation, node)?;
	declared.push(approval_decider()?);
	declared.push(aidash_domain::registry::bindings::source_defaults::workspace_observation()?);
	declared.push(compaction_decider()?);
	Ok(declared)
}

/// The built-in approval Decider. Its Routes follow ADR 0042's addendum: Jev
/// through System One, then Clef through OpenRouter's Decisions API on the three
/// hosts the Tenant's Catalog Approval names. Approving the version approves
/// sending a Run's state to each of them. It enforces, keeps no state and may
/// send 200 Decision Attempts per Run, about one per judged call.
///
/// The thresholds are not calibrated for this hook. `scripts/evaluate-decider-routes.py`
/// measured compaction only, so both Routes block at 0.3 until an evaluation on
/// approval fixtures chooses them; a changed threshold is a new version.
pub fn approval_decider() -> Result<Entry> {
	let config = DeciderConfig {
		hook: Hook::Approval,
		answer_type: AnswerType::Noul,
		description: "Blocks a pending tool call that sends data beyond the request, destroys or irreversibly changes things, goes beyond the request, follows content the Run read, or contradicts a stated boundary.".into(),
		provider_credential: "openrouter".into(),
		routes: vec![
			DeciderRoute {
				threshold: Probability::new(0.3)?,
				..DeciderRoute::jev("typesafe/jev-1.14")
			},
			DeciderRoute {
				provider_contract: DECISIONS.into(),
				model: "cloudflare/clef".into(),
				hosts: vec![
					"cloudflare".into(),
					"deepinfra".into(),
					"primeintellect".into(),
				],
				threshold: Probability::new(0.3)?,
				// The smallest of its hosts takes 16k tokens, and a byte is at
				// least a token, so 16,000 bytes stay inside it.
				state_budget: StateBudget {
					default_request_bytes: 16_000,
					default_state_bytes: 12_000,
					long_context_request_bytes: None,
					long_context_state_bytes: None,
					..StateBudget::default()
				},
			},
		],
		builder: Hook::Approval.contract().builder.into(),
		option_source: Hook::Approval.contract().option_source.into(),
		rule: Hook::Approval.contract().rule.into(),
		mode: Mode::Enforce,
		max_calls_per_run: 200,
		state_retention: StateRetention::default(),
	};
	config.validate()?;
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: APPROVAL_DECIDER_ID.into(),
		version: APPROVAL_DECIDER_VERSION.into(),
		kind: "decider".into(),
		name: std::collections::BTreeMap::from([("en".into(), "Approval Decider".into())]),
		description: std::collections::BTreeMap::from([("en".into(), config.description.clone())]),
		capabilities: vec![],
		tags: vec!["system".into()],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: serde_json::to_value(config)?,
	})
}

/// ADR 0042/0066's default pruning definition. Thresholds were measured on
/// compaction fixtures in `docs/decisions/decider-route-thresholds.md`.
/// Seeding this document grants neither Catalog Approval nor a credential.
pub fn compaction_decider() -> Result<Entry> {
	let config = DeciderConfig {
        hook: Hook::Compaction,
        answer_type: AnswerType::Noul,
        description: "Keep history needed to answer the current request while pruning unnecessary tool calls and results.".into(),
        provider_credential: "openrouter".into(),
        routes: vec![
            DeciderRoute::jev("typesafe/jev-1.14"),
            DeciderRoute {
                provider_contract: DECISIONS.into(),
                model: "cloudflare/clef".into(),
                hosts: vec!["cloudflare".into(), "deepinfra".into(), "primeintellect".into()],
                threshold: Probability::new(0.15)?,
                state_budget: StateBudget {
                    default_request_bytes: 16_000,
                    default_state_bytes: 12_000,
                    long_context_request_bytes: None,
                    long_context_state_bytes: None,
                    ..StateBudget::default()
                },
            },
        ],
        builder: Hook::Compaction.contract().builder.into(),
        option_source: Hook::Compaction.contract().option_source.into(),
        rule: Hook::Compaction.contract().rule.into(),
        mode: Mode::Enforce,
        max_calls_per_run: 100,
        state_retention: StateRetention::default(),
    };
	config.validate()?;
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: COMPACTION_DECIDER_ID.into(),
		version: COMPACTION_DECIDER_VERSION.into(),
		kind: "decider".into(),
		name: std::collections::BTreeMap::from([("en".into(), "Compaction Decider".into())]),
		description: std::collections::BTreeMap::from([("en".into(), config.description.clone())]),
		capabilities: vec![],
		tags: vec!["system".into()],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: serde_json::to_value(config)?,
	})
}

/// Writes every built-in definition into the scope's Tenant: the ordinary
/// built-ins, the approval and compaction Deciders, and the Sandboxed and research groups the
/// Deployment offers, which the Tenant approves before use. The caller owns one
/// transaction. Conflicting bytes fail.
pub async fn seed(
	scope: &mut dyn DefinitionWriter,
	validation: &DefinitionValidation,
	node: &str,
) -> Result<()> {
	let offered = packages::offered(validation, validation, node)?
		.into_iter()
		.flat_map(|group| group.operations.into_iter().chain([group.bundle]));
	let deciders = [approval_decider()?, compaction_decider()?];
	for entry in entries(validation, node)?
		.into_iter()
		.chain(legacy_workspace_entries(validation, node)?)
		.chain(offered)
		.chain(deciders)
		.chain([aidash_domain::registry::bindings::source_defaults::workspace_observation()?])
		.chain(external_services())
	{
		aidash_domain::registry::rules::validate_metadata(&entry, false)?;
		// Restart may precede recovery of a prepared transaction. Even an
		// INSERT that does nothing takes Registry's statement-level write guard.
		match scope.definition(&entry.id, &entry.version).await {
			Ok(saved) if saved == entry => continue,
			Ok(_) => {
				return Err(crate::Error::Conflict(
					"system declaration bytes changed".into(),
				));
			}
			Err(crate::Error::NotFound(_)) => {}
			Err(error) => return Err(error),
		}
		scope.insert_definition(&entry).await?;
	}
	Ok(())
}

pub fn is_builtin(entry: &Entry) -> bool {
	if entry.kind == "source" {
		return aidash_domain::registry::bindings::source_defaults::workspace_observation()
			.is_ok_and(|canonical| &canonical == entry);
	}
	entry.kind == "tool"
		&& serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
			entry.config.clone(),
		)
		.is_ok_and(|descriptor| {
			descriptor.tier == aidash_domain::tool::providers::ToolTier::Builtin
		})
}

pub fn builtin_reference(reference: &aidash_domain::registry::EntityRef) -> bool {
	(reference.version == "1.0.0"
		|| reference.version == "1.0.1"
			&& matches!(
				reference.id.as_str(),
				"aidash.workspace_read" | "aidash.workspace_observe" | "aidash.workspace_message"
			)) && (reference.id
		== aidash_domain::registry::bindings::source_defaults::WORKSPACE_OBSERVATION_ID
		|| reference
			.id
			.strip_prefix("aidash.")
			.is_some_and(|operation| operations().any(|builtin| builtin == operation)))
}

/// A definition the Deployment provides to every Tenant, which no Tenant changes:
/// its identifier carries a reserved prefix (ADR 0033).
pub fn is_deployment_definition(entry: &Entry) -> bool {
	entry.id.starts_with("aidash.") || entry.id.starts_with("core.")
}

pub fn reject_distribution(entry: &Entry) -> Result<()> {
	if is_deployment_definition(entry) {
		return Err(crate::Error::Invalid("system builtin declarations cannot be published, installed or mutated through Marketplace".into()));
	}
	Ok(())
}

pub fn reject_owner_definition(entry: &Entry) -> Result<()> {
	if entry.id.starts_with("aidash.")
		|| entry.id.starts_with("core.")
		|| entry.kind == "tool"
			&& serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
				entry.config.clone(),
			)
			.is_ok_and(|descriptor| {
				// Node providers are declared only by the Node's packages.
				descriptor.provider.starts_with("core.")
					|| descriptor.provider.starts_with("research.")
			}) {
		return Err(crate::Error::Forbidden);
	}
	Ok(())
}

pub fn attributes(entry: &Entry) -> serde_json::Value {
	let deployment = is_deployment_definition(entry);
	json!({"origin":if deployment {"system"} else {"owner"},"immutable":deployment})
}

pub mod packages;
#[cfg(test)]
mod tests;

/// Package indexes still require each Tenant's Catalog Approval and explicit Binding.
fn external_services() -> Vec<Entry> {
	[
		("npm", "https://registry.npmjs.org/"),
		("pypi", "https://pypi.org/"),
	]
	.into_iter()
	.map(|(name, base_url)| Entry {
		binding_normalization: None,
		installation: None,
		id: format!("core.{name}"),
		version: "1.0.0".into(),
		kind: "external_service".into(),
		name: std::collections::BTreeMap::from([("en".into(), name.into())]),
		description: std::collections::BTreeMap::from([(
			"en".into(),
			aidash_domain::egress::DISCLOSURE.into(),
		)]),
		capabilities: vec![],
		tags: vec!["system".into()],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: json!({"base_url":base_url,"methods":["GET","HEAD"],"authentication":{"type":"bearer"}}),
	})
	.collect()
}
