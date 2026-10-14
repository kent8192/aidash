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
pub use evidence::*;
pub use probability::Probability;

pub const PROVIDER: &str = "typesafe.jev/1";
pub const BUILDER: &str = "aidash.compaction/1";
pub const OPTION_SOURCE: &str = "history_event/1";
pub const RULE: &str = "compaction.keep/1";
pub const EVIDENCE_VERSION: u8 = 2;
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
		{
			return Err(Error::Invalid(
				"unsupported Decider contract or compaction rule".into(),
			));
		}
		validate_model(&self.model)
	}
	pub fn digest(&self) -> Result<String> {
		Ok(model_configuration_digest(
			&self.model,
			&self.parameters_digest()?,
		))
	}
	/// A replay witness for all non-model fields without disclosing transport references.
	pub fn parameters_digest(&self) -> Result<String> {
		let mut parameters = serde_json::to_value(self)?;
		parameters["model"] = serde_json::Value::Null;
		Ok(digest(&parameters))
	}
	pub fn contract_digest(&self) -> Result<String> {
		self.validate()?;
		Ok(digest(
			&serde_json::json!({"provider":self.provider_contract,"builder":self.builder,"options":self.option_source,"rule":self.rule}),
		))
	}
}

fn model_configuration_digest(model: &str, parameters_digest: &str) -> String {
	digest(&serde_json::json!({"model": model, "parameters_digest": parameters_digest}))
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
	Ok(())
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
	pub preserve_recent: usize,
	pub forbid_apply: bool,
}
impl Default for Restrictions {
	fn default() -> Self {
		Self {
			keep_threshold: Probability::half(),
			preserve_recent: 6,
			forbid_apply: false,
		}
	}
}
impl Restrictions {
	pub fn validate(&self, config: &DeciderConfig) -> Result<()> {
		config.validate()?;
		if self.keep_threshold.value() > config.keep_threshold.value() || self.preserve_recent < 6 {
			return Err(Error::Invalid(
				"Decision restrictions must preserve at least the declared history".into(),
			));
		}
		Ok(())
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
