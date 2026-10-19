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

/// Jev through OpenRouter's System One API.
pub const PROVIDER: &str = "typesafe.jev/1";
/// OpenRouter's Decisions API, which asks every decision model the same typed
/// questions on the hosts a Route names (ADR 0066).
pub const DECISIONS: &str = "openrouter.decisions/1";
pub const BUILDER: &str = "aidash.compaction/2";
pub const OPTION_SOURCE: &str = "history_event/1";
pub const RULE: &str = "compaction.keep/1";
pub const EVIDENCE_VERSION: u8 = 4;
/// Decider Routes one version may list.
pub const MAX_ROUTES: usize = 4;
/// OpenRouter hosts one Decisions Route may name.
pub const MAX_HOSTS: usize = 8;
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

/// A Decider reaches its models only through OpenRouter, with the Tenant's
/// Provider Credential for the named catalog entry (ADR 0006, ADR 0041), along
/// the Decider Routes its version lists in order (ADR 0066).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderConfig {
	pub hook: Hook,
	pub answer_type: AnswerType,
	pub description: String,
	/// The Provider Catalog entry whose Tenant binding supplies the credential
	/// of every Route.
	pub provider_credential: String,
	/// Tried in order; a later Route only when every earlier one is unavailable.
	pub routes: Vec<DeciderRoute>,
	pub builder: String,
	pub option_source: String,
	pub rule: String,
	pub mode: Mode,
	/// Decision Attempts one Run may send, on every Route together; a Binding
	/// can only lower it.
	pub max_calls_per_run: u64,
	/// Classification state kept with its evidence. A Catalog Approval of this
	/// exact version is what allows it.
	#[serde(default)]
	pub state_retention: policy::StateRetention,
}

/// One way a Decider version can be answered (ADR 0066). The System One Route
/// reaches Jev, which TypeSafe alone serves; a Decisions Route reaches its model
/// only on the OpenRouter hosts it names. Models calibrate their probabilities
/// differently, so each Route keeps its own threshold and state budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderRoute {
	pub provider_contract: String,
	pub model: String,
	/// OpenRouter host slugs allowed to run the model, sent as `provider.only`.
	/// Empty on the System One Route.
	#[serde(default)]
	pub hosts: Vec<String>,
	/// The probability at which this Route's answers keep content. A Binding's
	/// stricter threshold still applies.
	pub keep_threshold: Probability,
	/// How much state this Route sends and how it abridges it.
	#[serde(default)]
	pub state_budget: StateBudget,
}
impl DeciderRoute {
	/// Jev through System One at the 0.5 threshold and the default budget.
	pub fn jev(model: impl Into<String>) -> Self {
		Self {
			provider_contract: PROVIDER.into(),
			model: model.into(),
			hosts: vec![],
			keep_threshold: Probability::half(),
			state_budget: StateBudget::default(),
		}
	}
	pub fn validate(&self) -> Result<()> {
		self.state_budget.validate()?;
		let threshold = self.keep_threshold.value();
		if threshold <= 0.0 || threshold > 0.5 {
			return Err(Error::Invalid(
				"a Decider Route keeps content at a threshold above 0 and at most 0.5".into(),
			));
		}
		match self.provider_contract.as_str() {
			PROVIDER if self.hosts.is_empty() => validate_model(&self.model),
			DECISIONS if self.state_budget.long_context_request_bytes.is_none() => {
				validate_hosts(&self.hosts)?;
				validate_decision_model(&self.model)
			}
			_ => Err(Error::Invalid(
				"unsupported Decider Route contract, hosts or long-context mode".into(),
			)),
		}
	}
}

/// Exact OpenRouter host slugs such as `cloudflare` or `deepinfra`, each named
/// once, so a host OpenRouter adds later never receives state.
fn validate_hosts(hosts: &[String]) -> Result<()> {
	let slug = |host: &str| {
		!host.is_empty()
			&& host.len() <= 64
			&& host.split('-').all(|part| {
				!part.is_empty()
					&& part
						.bytes()
						.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
			})
	};
	let unique = hosts
		.iter()
		.collect::<std::collections::BTreeSet<_>>()
		.len() == hosts.len();
	if hosts.is_empty() || hosts.len() > MAX_HOSTS || !unique || !hosts.iter().all(|h| slug(h)) {
		return Err(Error::Invalid(
			"a Decisions Route names one to eight distinct OpenRouter host slugs".into(),
		));
	}
	Ok(())
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
			// Each long-context bound exceeds its default-mode bound, so
			// `max_request_bytes` bounds every request this version sends.
			(Some(request), Some(state)) => {
				state < request
					&& state > self.default_state_bytes
					&& request > self.default_request_bytes
					&& request <= 1_048_576
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
			|| self.builder != BUILDER
			|| self.option_source != OPTION_SOURCE
			|| self.rule != RULE
			|| !(1..=1_000).contains(&self.max_calls_per_run)
			|| !(1..=30).contains(&self.state_retention.days)
			|| self.routes.is_empty()
			|| self.routes.len() > MAX_ROUTES
		{
			return Err(Error::Invalid(
				"unsupported Decider contract or compaction rule".into(),
			));
		}
		for (index, route) in self.routes.iter().enumerate() {
			route.validate()?;
			if self.routes[..index].iter().any(|earlier| {
				earlier.provider_contract == route.provider_contract
					&& earlier.model == route.model
					&& earlier.hosts == route.hosts
			}) {
				return Err(Error::Invalid(
					"a Decider version lists the same Route twice".into(),
				));
			}
		}
		Ok(())
	}
	/// The models of the Routes, in order.
	pub fn models(&self) -> Vec<String> {
		self.routes
			.iter()
			.map(|route| route.model.clone())
			.collect()
	}
	/// Every Route's threshold in order, which replay checks a decision's
	/// restrictions against.
	pub fn thresholds(&self) -> Vec<Probability> {
		self.routes
			.iter()
			.map(|route| route.keep_threshold)
			.collect()
	}
	pub fn digest(&self) -> Result<String> {
		Ok(configuration_digest(
			&self.models(),
			&self.thresholds(),
			self.mode,
			&self.parameters_digest()?,
		))
	}
	/// A replay witness for all fields but the Route models and thresholds and
	/// the mode, which evidence records on its own, without disclosing transport
	/// references.
	pub fn parameters_digest(&self) -> Result<String> {
		let mut parameters = serde_json::to_value(self)?;
		for route in parameters["routes"].as_array_mut().into_iter().flatten() {
			route["model"] = serde_json::Value::Null;
			route["keep_threshold"] = serde_json::Value::Null;
		}
		parameters["mode"] = serde_json::Value::Null;
		Ok(digest(&parameters))
	}
	pub fn contract_digest(&self) -> Result<String> {
		self.validate()?;
		let providers: Vec<_> = self
			.routes
			.iter()
			.map(|route| route.provider_contract.as_str())
			.collect();
		Ok(digest(
			&serde_json::json!({"providers":providers,"builder":self.builder,"options":self.option_source,"rule":self.rule}),
		))
	}
	/// The largest request any Route may send; owners that cannot see the
	/// request charge this bound instead of its claimed size.
	pub fn max_request_bytes(&self) -> usize {
		self.routes
			.iter()
			.map(|route| route.state_budget.max_request_bytes())
			.max()
			.unwrap_or_default()
	}
}

/// Commits to the Route models and thresholds and the mode evidence records,
/// so a decision that sent no request still proves the mode its Decider ran
/// in, and replay proves the threshold of the Route it asked.
pub(crate) fn configuration_digest(
	models: &[String],
	thresholds: &[Probability],
	mode: Mode,
	parameters_digest: &str,
) -> String {
	digest(&serde_json::json!({
		"models": models,
		"thresholds": thresholds,
		"mode": mode,
		"parameters_digest": parameters_digest,
	}))
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

/// An exact OpenRouter model ID for a Decisions Route, such as
/// `cloudflare/clef`. Aliases (`~…`) move under a pinned version and variants
/// (`…:free`) route to other endpoints, so both are refused.
pub fn validate_decision_model(model: &str) -> Result<()> {
	let exact = model.split_once('/').is_some_and(|(maker, name)| {
		!maker.is_empty() && !name.is_empty() && !maker.starts_with('~')
	}) && !model.contains(':');
	if !exact {
		return Err(Error::Invalid(
			"a Decisions Route requires an exact OpenRouter model ID such as cloudflare/clef"
				.into(),
		));
	}
	crate::provider_credentials::validate_model_id(model)
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
	/// A Route's own threshold is intersected in when that Route is tried, so
	/// only the floor bounds the threshold here.
	pub fn validate(&self, config: &DeciderConfig) -> Result<()> {
		config.validate()?;
		self.validate_floor()?;
		if self
			.max_calls_per_run
			.is_some_and(|limit| limit > config.max_calls_per_run)
		{
			return Err(Error::Invalid(
				"Decision restrictions must preserve at least the declared history".into(),
			));
		}
		Ok(())
	}
	/// These restrictions with a Route's own threshold, whichever keeps more.
	pub fn with_route(&self, route: &DeciderRoute) -> Self {
		let mut restrictions = self.clone();
		if route.keep_threshold.value() < restrictions.keep_threshold.value() {
			restrictions.keep_threshold = route.keep_threshold;
		}
		restrictions
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
