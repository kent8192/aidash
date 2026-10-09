//! Deferred capability exposure: the Exposure policy, the catalog of
//! Discoverable capabilities, per-request budgeted selection and the durable
//! Load/Unload state. Everything here is pure and deterministic; ties are
//! broken by alias. Budgets are bytes of the JSON request text.
use crate::{
	Error, Result,
	capabilities::skills::SkillMetadata,
	context::estimated_tokens,
	provider::ToolSpec,
	registry::{
		Entry, Localized, SkillFile,
		bindings::{
			AgentBindings, BindingKind, BindingSnapshot, BundleConfig, DEFAULT_TOOLS,
			EXPOSURE_TOOLS, QualifiedRef, REQUIRED_TOOLS, SKILL_ASSET_READ, SKILL_TOOLS,
		},
		rules,
	},
	tool::providers::ToolDescriptor,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const DEFAULT_METADATA_BYTES: usize = 4096;
pub const DEFAULT_SCHEMA_BYTES: usize = 16384;
pub const DEFAULT_SKILL_BYTES: usize = 32768;
/// Results returned by one `capability_search` page.
pub const SEARCH_PAGE_SIZE: usize = 16;
const SUMMARY_CHARS: usize = 160;
const SKILL_STEM_CHARS: usize = 40;
const INDEX_HEADER: &str = "Discoverable capabilities that are not loaded. Use capability_search to find capabilities and capability_load to load one before use:\n";

/// How an Agent's bound capabilities reach the model. Absent means `legacy@1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "version", deny_unknown_fields, from = "PolicyFields")]
pub enum ExposurePolicy {
	#[default]
	#[serde(rename = "legacy@1")]
	Legacy,
	#[serde(rename = "deferred@1")]
	Deferred(DeferredBudgets),
}
/// Serde accepts any field beside the tag of an internally tagged unit
/// variant; an empty struct variant rejects them.
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "ExposurePolicy")]
#[serde(tag = "version", deny_unknown_fields)]
enum PolicyFields {
	#[serde(rename = "legacy@1")]
	Legacy {},
	#[serde(rename = "deferred@1")]
	Deferred(DeferredBudgets),
}
impl From<PolicyFields> for ExposurePolicy {
	fn from(fields: PolicyFields) -> Self {
		match fields {
			PolicyFields::Legacy {} => Self::Legacy,
			PolicyFields::Deferred(budgets) => Self::Deferred(budgets),
		}
	}
}
impl ExposurePolicy {
	pub fn validate(&self) -> Result<()> {
		match self {
			Self::Legacy => Ok(()),
			Self::Deferred(budgets) => budgets.validate(),
		}
	}
	pub fn budgets(&self) -> Option<&DeferredBudgets> {
		match self {
			Self::Legacy => None,
			Self::Deferred(budgets) => Some(budgets),
		}
	}
	pub fn is_deferred(&self) -> bool {
		matches!(self, Self::Deferred(_))
	}
	/// Canonical, non-removable operations normalized with origin `Required`.
	pub fn required_tools(&self) -> Vec<&'static str> {
		let mut tools = REQUIRED_TOOLS.to_vec();
		if self.is_deferred() {
			tools.extend_from_slice(EXPOSURE_TOOLS);
		}
		tools
	}
	/// Implicit defaults that `remove_default` may name.
	pub fn default_tools(&self) -> Vec<&'static str> {
		if self.is_deferred() {
			DEFAULT_TOOLS
				.iter()
				.copied()
				.filter(|name| !SKILL_TOOLS.contains(name))
				.chain([SKILL_ASSET_READ])
				.collect()
		} else {
			DEFAULT_TOOLS.to_vec()
		}
	}
	/// Defaults that bound Skills require and that receive origin `SkillSupport`.
	pub fn skill_support_tools(&self) -> &'static [&'static str] {
		if self.is_deferred() {
			&[SKILL_ASSET_READ]
		} else {
			SKILL_TOOLS
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferredBudgets {
	#[serde(default = "default_metadata_bytes")]
	pub metadata_bytes: usize,
	#[serde(default = "default_schema_bytes")]
	pub schema_bytes: usize,
	#[serde(default = "default_skill_bytes")]
	pub skill_bytes: usize,
}
fn default_metadata_bytes() -> usize {
	DEFAULT_METADATA_BYTES
}
fn default_schema_bytes() -> usize {
	DEFAULT_SCHEMA_BYTES
}
fn default_skill_bytes() -> usize {
	DEFAULT_SKILL_BYTES
}
impl Default for DeferredBudgets {
	fn default() -> Self {
		Self {
			metadata_bytes: DEFAULT_METADATA_BYTES,
			schema_bytes: DEFAULT_SCHEMA_BYTES,
			skill_bytes: DEFAULT_SKILL_BYTES,
		}
	}
}
impl DeferredBudgets {
	pub fn validate(&self) -> Result<()> {
		if !(512..=65_536).contains(&self.metadata_bytes)
			|| !(1024..=262_144).contains(&self.schema_bytes)
			|| !(1024..=262_144).contains(&self.skill_bytes)
		{
			return Err(Error::Invalid(
				"deferred exposure budgets are out of range".into(),
			));
		}
		Ok(())
	}
	fn for_kind(&self, kind: CapabilityKind) -> usize {
		match kind {
			CapabilityKind::Tool => self.schema_bytes,
			CapabilityKind::Skill => self.skill_bytes,
		}
	}
}

/// Per-Binding exposure under `deferred@1`. Absent means Deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingExposure {
	Eager,
	Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
	Tool,
	Skill,
}
impl CapabilityKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Tool => "tool",
			Self::Skill => "skill",
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillOriginKind {
	Registry,
	Attachment,
	Root,
}
impl SkillOriginKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Registry => "registry",
			Self::Attachment => "attachment",
			Self::Root => "root",
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CapabilityIdentity {
	Registry(QualifiedRef),
	DirectSkill { skill_id: Uuid, origin: String },
}
impl std::fmt::Display for CapabilityIdentity {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Registry(reference) => write!(
				f,
				"{}/{}@{}",
				reference.registry_node, reference.id, reference.version
			),
			Self::DirectSkill { skill_id, origin } => write!(f, "{origin}#{skill_id}"),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capability {
	pub alias: String,
	pub kind: CapabilityKind,
	pub identity: CapabilityIdentity,
	pub digest: String,
	pub name: String,
	pub description: String,
	pub bundle: Option<String>,
	/// Lifecycle companions (shell_poll/cancel…), exposed with the parent.
	pub companions: Vec<String>,
	/// Tool: serialized ToolSpec JSON length (+companions); Skill: escaped
	/// resident block length.
	pub definition_bytes: usize,
	/// Tool: the exact ToolSpec; Skill: {name, description, origin, license, files}.
	pub detail: Value,
	pub mandatory: bool,
	pub eager: bool,
}

/// A Skill mounted directly (attachment or root) rather than bound from the Registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectSkill {
	pub metadata: SkillMetadata,
	pub origin: SkillOriginKind,
	/// JSON-escaped byte length (without quotes) of the SKILL.md body, the
	/// text that becomes resident inside the request instructions.
	pub body_bytes: usize,
	/// `{path, digest, size}` per packaged file.
	pub files: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Loaded {
	pub alias: String,
	pub kind: CapabilityKind,
	pub identity: CapabilityIdentity,
	pub digest: String,
	pub step: i32,
}

/// Durable Exposure set changes of one Run, in load order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ExposureState {
	pub loaded: Vec<Loaded>,
	/// Aliases whose eager exposure was withdrawn. An Unload records every
	/// alias here; only Eager bindings consult it, and a Load removes it.
	pub unloaded_eager: BTreeSet<String>,
}
impl ExposureState {
	pub fn is_empty(&self) -> bool {
		self.loaded.is_empty() && self.unloaded_eager.is_empty()
	}
	/// Idempotent: loading a present alias and unloading an absent one are no-ops.
	pub fn apply(&mut self, update: &ExposureUpdate) {
		match update {
			ExposureUpdate::Load(loaded) => {
				self.unloaded_eager.remove(&loaded.alias);
				if let Some(index) = self.loaded.iter().position(|l| l.alias == loaded.alias) {
					let present = &self.loaded[index];
					if present.digest == loaded.digest && present.identity == loaded.identity {
						return;
					}
					// A changed capability replaces its stale load record.
					self.loaded.remove(index);
				}
				self.loaded.push(loaded.clone());
			}
			ExposureUpdate::Unload { alias } => {
				self.loaded.retain(|l| &l.alias != alias);
				self.unloaded_eager.insert(alias.clone());
			}
		}
	}
}

/// Serialized into tool output `"exposure_update"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ExposureUpdate {
	Load(Loaded),
	Unload { alias: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExposureUsage {
	pub metadata_bytes: usize,
	pub schema_bytes: usize,
	pub skill_bytes: usize,
	/// `(alias, digest)` of every exposed capability, by alias.
	pub exposed: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
	/// Exposed tool aliases, including lifecycle companions.
	pub tools: BTreeSet<String>,
	/// Resident Skill aliases: eager first, then loaded in load order.
	pub skills: Vec<String>,
	pub index: String,
	pub index_omitted: usize,
	pub usage: ExposureUsage,
}

/// Every Discoverable capability of a Run, sorted by alias. `specs` are the
/// tool specifications exactly as dispatch advertises them, keyed by alias.
pub fn catalog(
	snapshot: &BindingSnapshot,
	specs: &BTreeMap<String, ToolSpec>,
	direct: &[DirectSkill],
) -> Result<Vec<Capability>> {
	let agent = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.ok_or_else(|| Error::Invalid("snapshot lacks its Agent definition".into()))?;
	let config: AgentBindings = serde_json::from_value(agent.definition.config.clone())?;
	let definitions = snapshot
		.definitions
		.iter()
		.map(|definition| (&definition.identity, &definition.definition))
		.collect::<BTreeMap<_, _>>();
	// Bundle provenance and eagerness of every Tool reached through a bundle.
	let mut bundled = BTreeMap::<QualifiedRef, (String, bool)>::new();
	let mut explicit = BTreeMap::<&QualifiedRef, BindingExposure>::new();
	for binding in &config.bindings {
		let eager = binding.exposure == Some(BindingExposure::Eager);
		match binding.kind {
			BindingKind::Bundle => {
				let mut members = Vec::new();
				expand_bundle(
					&definitions,
					&binding.target,
					&binding.members,
					&mut BTreeSet::new(),
					&mut members,
				)?;
				for member in members {
					bundled
						.entry(member)
						.or_insert_with(|| (binding.target.id.clone(), false))
						.1 |= eager;
				}
			}
			BindingKind::Tool | BindingKind::Skill => {
				if let Some(exposure) = binding.exposure {
					explicit.insert(&binding.target, exposure);
				}
			}
			_ => {}
		}
	}
	let tools = snapshot
		.bindings
		.iter()
		.filter(|binding| binding.excluded_reason.is_none() && binding.definition.kind == "tool")
		.filter_map(|binding| {
			let alias = binding.alias.as_ref()?;
			specs.contains_key(alias).then_some((binding, alias))
		})
		.collect::<Vec<_>>();
	let aliases = tools
		.iter()
		.map(|(binding, alias)| (&binding.identity, *alias))
		.collect::<BTreeMap<_, _>>();
	let mut companions = BTreeMap::<&String, Vec<String>>::new();
	let mut folded = BTreeSet::new();
	for (binding, alias) in &tools {
		let descriptor: ToolDescriptor = serde_json::from_value(binding.definition.config.clone())?;
		if let Some(lifecycle) = descriptor.lifecycle {
			let mut found = [lifecycle.poll, lifecycle.cancel]
				.iter()
				.filter_map(|companion| aliases.get(companion).map(|alias| (*alias).clone()))
				.collect::<Vec<_>>();
			found.sort();
			folded.extend(found.iter().cloned());
			companions.insert(*alias, found);
		}
	}
	let mandatory = REQUIRED_TOOLS
		.iter()
		.chain(EXPOSURE_TOOLS)
		.chain(&[SKILL_ASSET_READ])
		.copied()
		.collect::<BTreeSet<_>>();
	let mut result = Vec::new();
	for (binding, alias) in &tools {
		if folded.contains(*alias) && !companions.contains_key(alias) {
			continue;
		}
		let spec = &specs[*alias];
		let companions = companions.remove(alias).unwrap_or_default();
		let mut definition_bytes = spec_bytes(spec)?;
		for companion in &companions {
			definition_bytes += spec_bytes(&specs[companion])?;
		}
		let bundle = bundled.get(&binding.identity);
		result.push(Capability {
			alias: (*alias).clone(),
			kind: CapabilityKind::Tool,
			identity: CapabilityIdentity::Registry(binding.identity.clone()),
			digest: binding.digest.clone(),
			name: localized(&binding.definition.name, &binding.identity.id),
			description: spec.description.clone(),
			bundle: bundle.map(|(id, _)| id.clone()),
			companions,
			definition_bytes,
			detail: serde_json::to_value(spec)?,
			mandatory: mandatory.contains(alias.as_str()),
			eager: match explicit.get(&binding.identity) {
				Some(exposure) => *exposure == BindingExposure::Eager,
				None => bundle.is_some_and(|(_, eager)| *eager),
			},
		});
	}
	for binding in snapshot
		.bindings
		.iter()
		.filter(|binding| binding.excluded_reason.is_none() && binding.definition.kind == "skill")
	{
		let identity = CapabilityIdentity::Registry(binding.identity.clone());
		let body = rules::skill_instructions(&binding.definition)?;
		let files = rules::skill_files(&binding.definition)?
			.iter()
			.map(file_detail)
			.collect::<Result<Vec<_>>>()?;
		let name = localized(&binding.definition.name, &binding.identity.id);
		let description = localized(&binding.definition.description, "");
		let mut capability = Capability {
			alias: skill_alias(&binding.identity.id, &identity),
			kind: CapabilityKind::Skill,
			identity,
			digest: binding.digest.clone(),
			detail: json!({
				"name": name,
				"description": description,
				"origin": SkillOriginKind::Registry,
				"license": binding.definition.config.get("license").cloned().unwrap_or(Value::Null),
				"files": files,
			}),
			name,
			description,
			bundle: None,
			companions: vec![],
			definition_bytes: 0,
			mandatory: false,
			eager: explicit.get(&binding.identity) == Some(&BindingExposure::Eager),
		};
		capability.definition_bytes = escaped_len(&resident_block(&capability, &body));
		result.push(capability);
	}
	for skill in direct {
		let metadata = &skill.metadata;
		let identity = CapabilityIdentity::DirectSkill {
			skill_id: metadata.skill_id,
			origin: metadata.origin.clone(),
		};
		let mut capability = Capability {
			alias: skill_alias(&metadata.name, &identity),
			kind: CapabilityKind::Skill,
			identity,
			digest: metadata.digest.clone(),
			name: metadata.name.clone(),
			description: metadata.description.clone(),
			bundle: None,
			companions: vec![],
			definition_bytes: 0,
			detail: json!({
				"name": metadata.name,
				"description": metadata.description,
				"origin": skill.origin,
				"license": metadata.license,
				"files": skill.files,
			}),
			mandatory: false,
			eager: false,
		};
		// JSON escaping is per character, so the block length is additive.
		capability.definition_bytes =
			escaped_len(&resident_block(&capability, "")) + skill.body_bytes;
		result.push(capability);
	}
	result.sort_by(|a, b| a.alias.cmp(&b.alias));
	if let Some(pair) = result
		.windows(2)
		.find(|pair| pair[0].alias == pair[1].alias)
	{
		return Err(Error::Conflict(format!(
			"duplicate capability alias: {}",
			pair[0].alias
		)));
	}
	Ok(result)
}

/// The resident instruction block of a Skill whose body is in the request.
pub fn resident_block(capability: &Capability, body: &str) -> String {
	format!(
		"\nSkill {} ({}; digest {}; origin {}):\n{body}\n",
		capability.alias,
		capability.identity,
		capability.digest,
		capability.detail["origin"].as_str().unwrap_or_default(),
	)
}

/// The Exposure set and index of one request.
pub fn select(
	budgets: &DeferredBudgets,
	catalog: &[Capability],
	state: &ExposureState,
) -> Result<Selection> {
	let mut tools = BTreeSet::new();
	let mut skills = Vec::new();
	let mut usage = ExposureUsage::default();
	let mut expose = |capability: &Capability, usage: &mut ExposureUsage| match capability.kind {
		CapabilityKind::Tool => {
			if tools.insert(capability.alias.clone()) {
				tools.extend(capability.companions.iter().cloned());
				usage.schema_bytes += capability.definition_bytes;
			}
		}
		CapabilityKind::Skill => {
			if !skills.contains(&capability.alias) {
				skills.push(capability.alias.clone());
				usage.skill_bytes += capability.definition_bytes;
			}
		}
	};
	for capability in catalog
		.iter()
		.filter(|c| c.mandatory || c.eager && !state.unloaded_eager.contains(&c.alias))
	{
		expose(capability, &mut usage);
	}
	if usage.schema_bytes > budgets.schema_bytes || usage.skill_bytes > budgets.skill_bytes {
		return Err(Error::Invalid(
			"mandatory and eager exposure exceeds the deferred budgets".into(),
		));
	}
	for loaded in &state.loaded {
		if let Some(capability) = find(catalog, &loaded.alias).filter(|c| current(c, loaded)) {
			expose(capability, &mut usage);
		}
	}
	let exposed = |capability: &&Capability| match capability.kind {
		CapabilityKind::Tool => tools.contains(&capability.alias),
		CapabilityKind::Skill => skills.contains(&capability.alias),
	};
	usage.exposed = catalog
		.iter()
		.filter(exposed)
		.map(|c| (c.alias.clone(), c.digest.clone()))
		.collect();
	let hidden = catalog.iter().filter(|c| !exposed(c)).collect::<Vec<_>>();
	let mut index = String::new();
	let mut index_omitted = 0;
	if !hidden.is_empty() {
		index.push_str(INDEX_HEADER);
		let mut used = escaped_len(INDEX_HEADER);
		let reserve = escaped_len(&footer(hidden.len()));
		for (position, capability) in hidden.iter().enumerate() {
			let line = format!(
				"{} [{}]: {}\n",
				capability.alias,
				capability.kind.as_str(),
				summary(&capability.description)
			);
			let line_bytes = escaped_len(&line);
			// Later lines need room for the footer that would replace them.
			let footer_bytes = if position + 1 < hidden.len() {
				reserve
			} else {
				0
			};
			if used + line_bytes + footer_bytes > budgets.metadata_bytes {
				index_omitted = hidden.len() - position;
				index.push_str(&footer(index_omitted));
				break;
			}
			used += line_bytes;
			index.push_str(&line);
		}
	}
	usage.metadata_bytes = escaped_len(&index);
	Ok(Selection {
		tools,
		skills,
		index,
		index_omitted,
		usage,
	})
}

/// Scored, paged discovery over the catalog.
pub fn search(
	catalog: &[Capability],
	state: &ExposureState,
	query: &str,
	cursor: Option<&str>,
) -> Result<Value> {
	let tokens = query
		.split(|c: char| !c.is_alphanumeric())
		.filter(|token| !token.is_empty())
		.map(str::to_lowercase)
		.collect::<Vec<_>>();
	let mut scored = catalog
		.iter()
		.map(|capability| {
			let fields = [
				(capability.alias.to_lowercase(), 2),
				(capability.name.to_lowercase(), 2),
				(capability.description.to_lowercase(), 1),
				(
					capability
						.bundle
						.as_deref()
						.unwrap_or_default()
						.to_lowercase(),
					1,
				),
			];
			let score = tokens
				.iter()
				.map(|token| {
					fields
						.iter()
						.filter(|(field, _)| field.contains(token.as_str()))
						.map(|(_, weight)| weight)
						.sum::<usize>()
				})
				.sum::<usize>();
			(score, capability)
		})
		.filter(|(score, _)| tokens.is_empty() || *score > 0)
		.collect::<Vec<_>>();
	scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.alias.cmp(&b.1.alias)));
	let offset = match cursor {
		None => 0,
		Some(cursor) => cursor
			.parse::<usize>()
			.ok()
			.filter(|offset| *offset <= scored.len())
			.ok_or_else(|| Error::Invalid("invalid capability_search cursor".into()))?,
	};
	let end = scored.len().min(offset + SEARCH_PAGE_SIZE);
	let truncated = end < scored.len();
	let results = scored[offset..end]
		.iter()
		.map(|(_, capability)| {
			json!({
				"alias": capability.alias,
				"kind": capability.kind,
				"name": capability.name,
				"description": summary(&capability.description),
				"digest": capability.digest,
				"loaded": is_exposed(capability, state),
			})
		})
		.collect::<Vec<_>>();
	Ok(json!({
		"results": results,
		"next_cursor": truncated.then(|| end.to_string()),
		"truncated": truncated,
	}))
}

/// Full detail of one capability; `budget` is the budget its bytes count against.
pub fn describe(catalog: &[Capability], budgets: &DeferredBudgets, alias: &str) -> Result<Value> {
	let capability = known(catalog, alias)?;
	Ok(json!({
		"alias": capability.alias,
		"kind": capability.kind,
		"identity": capability.identity,
		"digest": capability.digest,
		"detail": capability.detail,
		"bytes": capability.definition_bytes,
		"budget": budgets.for_kind(capability.kind),
	}))
}

/// Load a capability into the Exposure set without evicting anything.
pub fn load(
	budgets: &DeferredBudgets,
	catalog: &[Capability],
	state: &ExposureState,
	alias: &str,
	expected_digest: &str,
	step: i32,
) -> Result<Value> {
	let capability = known(catalog, alias)?;
	if capability.digest != expected_digest {
		return Err(Error::Invalid("CAPABILITY_CHANGED".into()));
	}
	if is_exposed(capability, state) {
		return Ok(json!({
			"status": "already_loaded",
			"identity": capability.identity,
			"digest": capability.digest,
		}));
	}
	let update = ExposureUpdate::Load(Loaded {
		alias: capability.alias.clone(),
		kind: capability.kind,
		identity: capability.identity.clone(),
		digest: capability.digest.clone(),
		step,
	});
	let mut next = state.clone();
	next.apply(&update);
	let used = |selection: &Selection| match capability.kind {
		CapabilityKind::Tool => selection.usage.schema_bytes,
		CapabilityKind::Skill => selection.usage.skill_bytes,
	};
	let required = used(&select(budgets, catalog, &next)?);
	let budget = budgets.for_kind(capability.kind);
	if required > budget {
		let exposed = catalog
			.iter()
			.filter(|c| c.kind == capability.kind && is_exposed(c, state))
			.map(|c| json!({"alias": c.alias, "bytes": c.definition_bytes}))
			.collect::<Vec<_>>();
		return Ok(json!({
			"error": "EXPOSURE_BUDGET_EXCEEDED",
			"exposed": exposed,
			"budget": budget,
			"required": required,
		}));
	}
	Ok(json!({
		"status": "loaded",
		"alias": capability.alias,
		"identity": capability.identity,
		"digest": capability.digest,
		"bytes": capability.definition_bytes,
		"exposure_update": update,
	}))
}

/// Withdraw a loaded or eager capability from the Exposure set.
pub fn unload(catalog: &[Capability], state: &ExposureState, alias: &str) -> Result<Value> {
	let capability = known(catalog, alias)?;
	if capability.mandatory {
		return Err(Error::Invalid("MANDATORY_EXPOSURE".into()));
	}
	if !is_exposed(capability, state) {
		return Ok(json!({"status": "not_loaded"}));
	}
	Ok(json!({
		"status": "unloaded",
		"exposure_update": ExposureUpdate::Unload {
			alias: capability.alias.clone(),
		},
	}))
}

fn find<'a>(catalog: &'a [Capability], alias: &str) -> Option<&'a Capability> {
	catalog.iter().find(|capability| capability.alias == alias)
}
fn known<'a>(catalog: &'a [Capability], alias: &str) -> Result<&'a Capability> {
	find(catalog, alias).ok_or_else(|| Error::Invalid("UNKNOWN_CAPABILITY".into()))
}
fn current(capability: &Capability, loaded: &Loaded) -> bool {
	capability.kind == loaded.kind
		&& capability.identity == loaded.identity
		&& capability.digest == loaded.digest
}
fn is_exposed(capability: &Capability, state: &ExposureState) -> bool {
	capability.mandatory
		|| capability.eager && !state.unloaded_eager.contains(&capability.alias)
		|| state
			.loaded
			.iter()
			.any(|loaded| loaded.alias == capability.alias && current(capability, loaded))
}

fn expand_bundle(
	definitions: &BTreeMap<&QualifiedRef, &Entry>,
	target: &QualifiedRef,
	selected: &[String],
	ancestry: &mut BTreeSet<QualifiedRef>,
	members: &mut Vec<QualifiedRef>,
) -> Result<()> {
	if !ancestry.insert(target.clone()) {
		return Err(Error::Invalid("recursive snapshot bundle expansion".into()));
	}
	let entry = definitions
		.get(target)
		.ok_or_else(|| Error::Invalid("snapshot lacks a bound definition".into()))?;
	let bundle: BundleConfig = serde_json::from_value(entry.config.clone())?;
	for member in bundle
		.members
		.into_iter()
		.filter(|member| selected.is_empty() || selected.contains(&member.id))
	{
		if definitions
			.get(&member)
			.is_some_and(|entry| entry.kind == "bundle")
		{
			expand_bundle(definitions, &member, &[], ancestry, members)?;
		} else {
			members.push(member);
		}
	}
	ancestry.remove(target);
	Ok(())
}

fn spec_bytes(spec: &ToolSpec) -> Result<usize> {
	Ok(estimated_tokens(&serde_json::to_string(spec)?))
}

fn localized(values: &Localized, fallback: &str) -> String {
	values
		.get("en")
		.or_else(|| values.values().next())
		.cloned()
		.unwrap_or_else(|| fallback.into())
}

fn summary(description: &str) -> String {
	description
		.lines()
		.next()
		.unwrap_or_default()
		.trim()
		.chars()
		.take(SUMMARY_CHARS)
		.collect()
}

fn footer(omitted: usize) -> String {
	format!("{omitted} more — use capability_search\n")
}

fn skill_alias(name: &str, identity: &CapabilityIdentity) -> String {
	let stem = name
		.chars()
		.map(|c| match c.to_ascii_lowercase() {
			c @ ('a'..='z' | '0'..='9' | '_') => c,
			_ => '_',
		})
		.take(SKILL_STEM_CHARS)
		.collect::<String>();
	let hash = format!("{:x}", Sha256::digest(identity.to_string().as_bytes()));
	format!("skill_{stem}_{}", &hash[..8])
}

fn file_detail(file: &SkillFile) -> Result<Value> {
	let bytes = match file.encoding.as_deref() {
		Some("base64") => {
			use base64::Engine;
			base64::engine::general_purpose::STANDARD
				.decode(&file.content)
				.map_err(|_| Error::Invalid(format!("{} has invalid base64 content", file.path)))?
		}
		_ => file.content.as_bytes().to_vec(),
	};
	Ok(json!({
		"path": file.path,
		"digest": format!("sha256:{:x}", Sha256::digest(&bytes)),
		"size": bytes.len(),
	}))
}

/// Byte length of `text` inside a JSON string, excluding the quotes.
fn escaped_len(text: &str) -> usize {
	text.chars()
		.map(|c| match c {
			'"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
			c if (c as u32) < 0x20 => 6,
			c => c.len_utf8(),
		})
		.sum()
}

#[cfg(test)]
mod tests;
