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
pub const EVIDENCE_VERSION: u8 = 1;
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

/// Transport credentials are local references, never part of request/evidence payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderConfig {
	pub hook: Hook,
	pub answer_type: AnswerType,
	pub description: String,
	pub provider_contract: String,
	pub endpoint: String,
	pub model: String,
	pub credential_env: String,
	pub builder: String,
	pub option_source: String,
	pub rule: String,
	pub keep_threshold: Probability,
	pub mode: Mode,
}
impl DeciderConfig {
	pub fn validate(&self) -> Result<()> {
		crate::configuration::validate_endpoint(&self.endpoint)?;
		crate::configuration::validate_secret_reference(&self.credential_env)?;
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
		Ok(digest(&serde_json::to_value(self)?))
	}
	pub fn contract_digest(&self) -> Result<String> {
		self.validate()?;
		Ok(digest(
			&serde_json::json!({"provider":self.provider_contract,"builder":self.builder,"options":self.option_source,"rule":self.rule}),
		))
	}
}

pub fn validate_model(model: &str) -> Result<()> {
	let version = model
		.strip_prefix("jev-")
		.and_then(|v| semver::Version::parse(v).ok())
		.filter(|v| v.pre.is_empty() && v.build.is_empty());
	if version.as_ref().is_none_or(|v| model != format!("jev-{v}")) {
		return Err(Error::Invalid(
			"Decider requires a concrete Jev model version".into(),
		));
	}
	Ok(())
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
