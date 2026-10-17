//! Explicit probability declarations and deterministic historical branch evidence.
use crate::{
	Error, Result,
	registry::{bindings::QualifiedRef, rules::digest},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod evidence;
pub mod policy;
pub mod probability;
pub mod pruning;
pub use evidence::*;
pub use probability::Probability;

pub const PROVIDER: &str = "typesafe.jev/1";
pub const BUILDER: &str = "aidash.compaction/2";
pub const OPTION_SOURCE: &str = "history_event/1";
pub const RULE: &str = "compaction.keep/1";
pub const EVIDENCE_VERSION: u8 = 3;
pub const INVOKE: &str = "decision.invoke";
pub const EVIDENCE_READ: &str = "decision.evidence.read";
pub const STATE_READ: &str = "decision.state.read";

#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Hook {
	Compaction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerType {
	Noul,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
	Shadow,
	Enforce,
}

/// A Decider reaches Jev only through OpenRouter's System One API, with the
/// Tenant's Provider Credential for the named catalog entry (ADR 0006, ADR 0041).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderConfig {
	pub hook: Hook,
	pub answer_type: AnswerType,
	pub description: String,
	pub provider_contract: String,
	/// The Provider Catalog entry whose Tenant binding supplies the credential.
	pub provider_credential: String,
	pub model: String,
	pub builder: String,
	pub option_source: String,
	pub rule: String,
	pub keep_threshold: Probability,
	pub mode: Mode,
	/// Decision Attempts one Run may send; a Binding can only lower it.
	pub max_calls_per_run: u64,
	/// Classification state kept with its evidence. A Catalog Approval of this
	/// exact version is what allows it.
	#[serde(default)]
	pub state_retention: policy::StateRetention,
	/// How much state this version sends to Jev and how it abridges it.
	#[serde(default)]
	pub state_budget: StateBudget,
}

/// Jev's default mode takes requests up to about 32,000 bytes (32k tokens for
/// the state plus one question). OpenRouter switches larger requests to the
/// long-context mode, up to 262,144 tokens, which TypeSafe describes as slower
/// and less accurate. A version therefore abridges tool inputs in stages to stay
/// in the default mode, and only a state that still does not fit after the
/// last stage may use the long-context mode (ADR 0042, 2026-10-17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StateBudget {
	/// Largest request, state and questions together, sent in the default mode.
	pub default_request_bytes: usize,
	/// Largest state sent in the default mode, leaving room for its questions.
	pub default_state_bytes: usize,
	/// Largest request in the long-context mode; `None` never uses it.
	pub long_context_request_bytes: Option<usize>,
	/// Largest state in the long-context mode.
	pub long_context_state_bytes: Option<usize>,
	/// Character limits for each disclosed tool input, tried in order; `None`
	/// sends the complete input.
	pub input_stages: Vec<Option<usize>>,
}
impl Default for StateBudget {
	fn default() -> Self {
		Self {
			default_request_bytes: 32_000,
			default_state_bytes: 24_000,
			long_context_request_bytes: Some(600_000),
			long_context_state_bytes: Some(560_000),
			input_stages: vec![None, Some(200), Some(60)],
		}
	}
}
impl StateBudget {
	pub fn validate(&self) -> Result<()> {
		let stages_shrink = self.input_stages.windows(2).all(|pair| match pair {
			[None, Some(_)] => true,
			[Some(first), Some(second)] => second < first,
			_ => false,
		});
		let long_context = match (
			self.long_context_request_bytes,
			self.long_context_state_bytes,
		) {
			(None, None) => true,
			(Some(request), Some(state)) => {
				state < request && state > self.default_state_bytes && request <= 1_048_576
			}
			_ => false,
		};
		if self.default_state_bytes == 0
			|| self.default_state_bytes >= self.default_request_bytes
			|| self.default_request_bytes > 32_000
			|| self.input_stages.is_empty()
			|| !stages_shrink
			|| self.input_stages.iter().flatten().any(|limit| *limit < 20)
			|| !long_context
		{
			return Err(Error::Invalid("unsupported Decider state budget".into()));
		}
		Ok(())
	}
	/// The largest request this version may send; owners that cannot see the
	/// request charge this bound instead of its claimed size.
	pub fn max_request_bytes(&self) -> usize {
		self.long_context_request_bytes
			.unwrap_or(self.default_request_bytes)
	}
	/// The largest request a state of `state_bytes` may travel in, or `None`
	/// when it fits no mode this version allows.
	pub fn request_limit(&self, state_bytes: usize) -> Option<usize> {
		if state_bytes <= self.default_state_bytes {
			Some(self.default_request_bytes)
		} else if self
			.long_context_state_bytes
			.is_some_and(|limit| state_bytes <= limit)
		{
			self.long_context_request_bytes
		} else {
			None
		}
	}
}
impl DeciderConfig {
	pub fn validate(&self) -> Result<()> {
		validate_provider_credential(&self.provider_credential)?;
		if self.description.trim().is_empty()
			|| self.provider_contract != PROVIDER
			|| self.builder != BUILDER
			|| self.option_source != OPTION_SOURCE
			|| self.rule != RULE
			|| self.keep_threshold.value() != 0.5
			|| !(1..=1_000).contains(&self.max_calls_per_run)
			|| !(1..=30).contains(&self.state_retention.days)
		{
			return Err(Error::Invalid(
				"unsupported Decider contract or compaction rule".into(),
			));
		}
		self.state_budget.validate()?;
		validate_model(&self.model)
	}
	pub fn digest(&self) -> Result<String> {
		Ok(model_configuration_digest(
			&self.model,
			self.mode,
			&self.parameters_digest()?,
		))
	}
	/// A replay witness for all fields but the model and mode, which evidence
	/// records on its own, without disclosing transport references.
	pub fn parameters_digest(&self) -> Result<String> {
		let mut parameters = serde_json::to_value(self)?;
		parameters["model"] = serde_json::Value::Null;
		parameters["mode"] = serde_json::Value::Null;
		Ok(digest(&parameters))
	}
	pub fn contract_digest(&self) -> Result<String> {
		self.validate()?;
		Ok(digest(
			&serde_json::json!({"provider":self.provider_contract,"builder":self.builder,"options":self.option_source,"rule":self.rule}),
		))
	}
}

/// Commits to the model and mode evidence records, so a decision that sent no
/// request still proves the mode its Decider ran in.
fn model_configuration_digest(model: &str, mode: Mode, parameters_digest: &str) -> String {
	digest(
		&serde_json::json!({"model": model, "mode": mode, "parameters_digest": parameters_digest}),
	)
}

/// Jev is reached only through OpenRouter (ADR 0041).
pub fn validate_provider_credential(provider: &str) -> Result<()> {
	if crate::provider_credentials::Provider::parse(provider)?
		!= crate::provider_credentials::Provider::Openrouter
	{
		return Err(Error::Invalid(
			"Jev is reached only through the OpenRouter Provider Credential".into(),
		));
	}
	Ok(())
}

/// An exact OpenRouter Jev release such as `typesafe/jev-1.13`. Aliases such as
/// `~typesafe/jev-latest` move under a pinned version, so they are refused.
/// A Capability Token must also be able to carry it.
pub fn validate_model(model: &str) -> Result<()> {
	let canonical = |part: &str| {
		!part.is_empty()
			&& part.bytes().all(|b| b.is_ascii_digit())
			&& (part == "0" || !part.starts_with('0'))
	};
	let exact = model
		.strip_prefix("typesafe/jev-")
		.and_then(|version| version.split_once('.'))
		.is_some_and(|(major, minor)| canonical(major) && canonical(minor));
	if !exact {
		return Err(Error::Invalid(
			"Jev requires an exact OpenRouter model version such as typesafe/jev-1.13".into(),
		));
	}
	crate::provider_credentials::validate_model_id(model)
}

/// OpenRouter reports the dated snapshot that served a pinned release, such as
/// `typesafe/jev-1.13-20260917` for `typesafe/jev-1.13`.
pub fn served_by(pinned: &str, served: &str) -> bool {
	served == pinned
		|| served
			.strip_prefix(pinned)
			.and_then(|rest| rest.strip_prefix('-'))
			.is_some_and(|date| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
}

/// Lower keep thresholds and larger protected suffixes preserve more content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Restrictions {
	pub keep_threshold: Probability,
	/// Events, or on the Native Projection Version Model Turns, never classified.
	pub preserve_recent: usize,
	pub forbid_apply: bool,
	/// A lower per-Run call limit than the Decider version declares.
	pub max_calls_per_run: Option<u64>,
}
impl Default for Restrictions {
	fn default() -> Self {
		Self {
			keep_threshold: Probability::half(),
			preserve_recent: 6,
			forbid_apply: false,
			max_calls_per_run: None,
		}
	}
}
impl Restrictions {
	/// The floor every Decider version shares, checked even where the version
	/// itself is not at hand.
	pub fn validate_floor(&self) -> Result<()> {
		if self.keep_threshold.value() > 0.5
			|| self.preserve_recent < 6
			|| self.max_calls_per_run == Some(0)
		{
			return Err(Error::Invalid(
				"Decision restrictions must preserve at least the declared history".into(),
			));
		}
		Ok(())
	}
	pub fn validate(&self, config: &DeciderConfig) -> Result<()> {
		config.validate()?;
		self.validate_floor()?;
		if self.keep_threshold.value() > config.keep_threshold.value()
			|| self
				.max_calls_per_run
				.is_some_and(|limit| limit > config.max_calls_per_run)
		{
			return Err(Error::Invalid(
				"Decision restrictions must preserve at least the declared history".into(),
			));
		}
		Ok(())
	}
	/// Whether these restrictions keep everything `other` keeps.
	pub fn at_least_as_strict(&self, other: &Self) -> bool {
		self.keep_threshold.value() <= other.keep_threshold.value()
			&& self.preserve_recent >= other.preserve_recent
			&& (self.forbid_apply || !other.forbid_apply)
			&& match (self.max_calls_per_run, other.max_calls_per_run) {
				(_, None) => true,
				(Some(mine), Some(theirs)) => mine <= theirs,
				(None, Some(_)) => false,
			}
	}
	pub fn intersect(&self, current: &Self, config: &DeciderConfig) -> Result<Self> {
		self.validate(config)?;
		current.validate(config)?;
		Ok(Self {
			keep_threshold: if self.keep_threshold.value() <= current.keep_threshold.value() {
				self.keep_threshold
			} else {
				current.keep_threshold
			},
			preserve_recent: self.preserve_recent.max(current.preserve_recent),
			forbid_apply: self.forbid_apply || current.forbid_apply,
			max_calls_per_run: match (self.max_calls_per_run, current.max_calls_per_run) {
				(Some(mine), Some(theirs)) => Some(mine.min(theirs)),
				(mine, theirs) => mine.or(theirs),
			},
		})
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderPin {
	pub identity: QualifiedRef,
	pub definition_digest: String,
	pub configuration_digest: String,
	/// Exact opaque Node adapter identity retained from Binding admission.
	pub provider_implementation: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct BoundDecider {
	pub pin: DeciderPin,
	pub config: DeciderConfig,
	pub restrictions: Restrictions,
}
impl BoundDecider {
	/// The admitted pin and the per-Run call limit its Binding leaves.
	pub fn allowance(&self) -> policy::ExecutionAllowance {
		policy::ExecutionAllowance {
			decider: self.pin.clone(),
			max_calls_per_run: self
				.restrictions
				.max_calls_per_run
				.map_or(self.config.max_calls_per_run, |limit| {
					limit.min(self.config.max_calls_per_run)
				}),
		}
	}
}
impl DeciderPin {
	pub fn validate(&self) -> Result<()> {
		self.identity.validate()?;
		if self.provider_implementation.trim().is_empty() {
			return Err(Error::Invalid(
				"Decider pin lacks a Node Provider implementation".into(),
			));
		}
		validate_digest(&self.definition_digest)?;
		validate_digest(&self.configuration_digest)
	}
	pub fn check(&self, definition: &crate::registry::Entry) -> Result<DeciderConfig> {
		self.validate()?;
		let config: DeciderConfig = serde_json::from_value(definition.config.clone())?;
		config.validate()?;
		if definition.kind != "decider"
			|| definition.id != self.identity.id
			|| definition.version != self.identity.version
			|| digest(&serde_json::to_value(definition)?) != self.definition_digest
			|| config.digest()? != self.configuration_digest
		{
			return Err(Error::Invalid(
				"Decider identity or approved configuration changed".into(),
			));
		}
		Ok(config)
	}
}
pub fn validate_digest(value: &str) -> Result<()> {
	let hex = value.strip_prefix("sha256:").unwrap_or_default();
	if hex.len() != 64
		|| !hex
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	{
		return Err(Error::Invalid(
			"decision digest must be sha256: followed by 64 lowercase hex digits".into(),
		));
	}
	Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Question {
	pub description: String,
	pub answer_type: AnswerType,
}
pub type Questions = BTreeMap<String, Question>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Branch {
	Keep,
	TruncateResult,
	Drop,
}
pub fn branch(keep_call: Probability, keep_result: Probability, threshold: Probability) -> Branch {
	if keep_result.value() >= threshold.value() {
		Branch::Keep
	} else if keep_call.value() >= threshold.value() {
		Branch::TruncateResult
	} else {
		Branch::Drop
	}
}

#[cfg(test)]
mod tests;
