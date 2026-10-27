//! Decider adapter for the Routes of one Decider version: Jev through
//! OpenRouter's System One API, other decision models through OpenRouter's
//! Decisions API on the hosts a Route names, all with the Tenant's Provider
//! Credential (ADR 0041, ADR 0066); no environment-selected fallback. Requests
//! stay within the byte budget of the Route they travel on.
use crate::{Error, Result};
use aidash_application::{
	ports::decision::{
		DecisionProvider, DispatchError, PreparedDispatch, PreparedRequest, ReportedDispatch,
	},
	provider_access::{Context, Inference, Operation, ProviderAccess, Source},
};
use aidash_domain::{decision::*, provider_credentials::Provider};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

/// Implemented by the Node's versioned provider/tokenizer capability catalog.
/// Restrictions here must describe the external service, never an Aidash ceiling.
pub trait RouteCapacity: Send + Sync {
	fn fits(
		&self,
		route: &DeciderRoute,
		state: &Value,
		questions: &Questions,
		encoded: &[u8],
	) -> Result<bool>;
}

/// A Route's capacity as its Decider version declares it: a request may be as
/// large as the mode its state travels in allows (ADR 0042, 2026-10-17).
pub struct ByteBudget;
impl RouteCapacity for ByteBudget {
	fn fits(
		&self,
		route: &DeciderRoute,
		state: &Value,
		_: &Questions,
		encoded: &[u8],
	) -> Result<bool> {
		let state = serde_json::to_vec(state)?.len();
		Ok(route
			.state_budget
			.request_limit(state)
			.is_some_and(|limit| encoded.len() <= limit))
	}
}

pub struct RouteDecisionProvider {
	config: DeciderConfig,
	implementation: String,
	client: reqwest::Client,
	access: Arc<dyn ProviderAccess>,
	/// The Run and its Tenant; each dispatch adds its own Decision Attempt scope.
	context: Context,
	capacity: Arc<dyn RouteCapacity>,
}
impl RouteDecisionProvider {
	/// The trusted Node catalog supplies this concrete implementation's opaque identity,
	/// independently of the publisher's configuration or a retained Run's requested pin.
	pub fn new(
		config: DeciderConfig,
		implementation: String,
		access: Arc<dyn ProviderAccess>,
		context: Context,
		capacity: Arc<dyn RouteCapacity>,
		timeout: Duration,
	) -> Result<Self> {
		config.validate()?;
		if context.tenant.is_empty()
			|| match config.hook {
				Hook::ApprovalObservation => {
					context.run.is_some() || context.observation.is_none_or(|id| id.is_nil())
				}
				_ => context.run.is_none_or(|run| run.is_nil()) || context.observation.is_some(),
			} || context.maintenance.is_some()
		{
			return Err(Error::Invalid(
				"Decider calls require one matching Run or approval observation scope in their Tenant".into(),
			));
		}
		if implementation.trim().is_empty() {
			return Err(Error::Invalid(
				"decision adapter lacks a Node implementation identity".into(),
			));
		}
		if timeout.is_zero() {
			return Err(Error::Invalid(
				"decision HTTP timeout must be positive".into(),
			));
		}
		// Redirects/retries create unreserved physical attempts or move disclosure.
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.retry(reqwest::retry::never())
			.timeout(timeout)
			.build()
			.map_err(|_| Error::Invalid("could not construct decision HTTP client".into()))?;
		Ok(Self {
			config,
			implementation,
			client,
			access,
			context,
			capacity,
		})
	}
	fn route(&self, route: usize) -> Result<&DeciderRoute> {
		self.config
			.routes
			.get(route)
			.ok_or_else(|| Error::Invalid("the Decider version has no such Route".into()))
	}
	fn encode(
		&self,
		route: &DeciderRoute,
		state: &Value,
		questions: &Questions,
	) -> Result<PreparedRequest> {
		let questions_json: BTreeMap<_, _> = questions
			.iter()
			.map(|(key, q)| (key, json!({"type":"noul", "instructions":q.description})))
			.collect();
		// Zero data retention is required on every OpenRouter call. A Decisions
		// Route also names the only hosts it approved, so OpenRouter routes
		// among them and never to a host added later (ADR 0066).
		let provider = if route.provider_contract == DECISIONS {
			json!({"zdr":true,"only":route.hosts,"allow_fallbacks":true})
		} else {
			json!({"zdr":true})
		};
		Ok(PreparedRequest {
			body: serde_json::to_vec(&json!({
				"model":route.model,"state":state,"questions":questions_json,
				"provider":provider
			}))?,
			questions: questions.clone(),
		})
	}
}
#[async_trait]
impl DecisionProvider for RouteDecisionProvider {
	fn implementation_id(&self) -> &str {
		&self.implementation
	}
	fn configuration_digest(&self) -> Result<String> {
		Ok(self.config.digest()?)
	}
	fn plan(
		&self,
		route: usize,
		state: &Value,
		questions: &Questions,
	) -> Result<Vec<PreparedRequest>> {
		if questions.is_empty() || questions.values().any(|q| q.description.trim().is_empty()) {
			return Err(Error::Invalid(
				"decision requires complete described Noul questions".into(),
			));
		}
		let route = self.route(route)?;
		let complete = self.encode(route, state, questions)?;
		if self
			.capacity
			.fits(route, state, questions, &complete.body)?
		{
			return Ok(vec![complete]);
		}
		let mut batches = vec![];
		let mut pending = Questions::new();
		let mut last = None;
		for (id, question) in questions {
			pending.insert(id.clone(), question.clone());
			let request = self.encode(route, state, &pending)?;
			if self.capacity.fits(route, state, &pending, &request.body)? {
				last = Some(request);
				continue;
			}
			pending.remove(id);
			let Some(previous) = last.take() else {
				return Err(Error::Invalid(
					"complete decision state and question exceed external provider capacity".into(),
				));
			};
			batches.push(previous);
			pending.clear();
			pending.insert(id.clone(), question.clone());
			let request = self.encode(route, state, &pending)?;
			if !self.capacity.fits(route, state, &pending, &request.body)? {
				return Err(Error::Invalid(
					"complete decision state and question exceed external provider capacity".into(),
				));
			}
			last = Some(request);
		}
		if let Some(request) = last {
			batches.push(request);
		}
		Ok(batches)
	}
	async fn prepare(
		&self,
		route: usize,
		request: &PreparedRequest,
		attempt: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		let route = self.route(route)?;
		let body: Value = serde_json::from_slice(&request.body)
			.map_err(|_| Error::Invalid("invalid prepared decision payload".into()))?;
		let state = body
			.get("state")
			.ok_or_else(|| Error::Invalid("prepared decision has no state".into()))?;
		if request.questions.is_empty()
			|| self.encode(route, state, &request.questions)?.body != request.body
		{
			return Err(Error::Invalid(
				"prepared decision payload changed its pinned model or questions".into(),
			));
		}
		let mut scope = self.context.clone();
		scope.provider_credential_id = None;
		scope.inference = Some(Inference {
			provider_tools: vec![],
			model: route.model.clone(),
			operations: vec![Operation::Decision],
			max_output_tokens: 0,
			cache_breakpoints: false,
			call: Some(attempt),
			hosts: route.hosts.clone(),
		});
		let source = Source::Tenant {
			provider: self.config.provider_credential.clone(),
		};
		// Checked before the attempt is reserved, so a missing or revoked
		// credential never charges an attempt that sends no request. The check
		// neither mints a token nor reads Key Material.
		self.access
			.check(&scope, Provider::Openrouter.base_url(), &source)
			.await?;
		Ok(Box::new(RouteDispatch {
			client: self.client.clone(),
			access: self.access.clone(),
			scope,
			source,
			body: request.body.clone(),
			attempt,
			route: route.clone(),
			questions: request.questions.clone(),
		}))
	}
}

struct RouteDispatch {
	client: reqwest::Client,
	access: Arc<dyn ProviderAccess>,
	/// The Decision Attempt's call scope and the Tenant binding it uses.
	scope: Context,
	source: Source,
	body: Vec<u8>,
	/// The Decision Attempt this request is; a direct (self-hosted) call has no
	/// broker audit, so the response is traced under it.
	attempt: Uuid,
	route: DeciderRoute,
	questions: Questions,
}
#[async_trait]
impl PreparedDispatch for RouteDispatch {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
		self.reported().await.result
	}
	async fn dispatch_reported(self: Box<Self>) -> ReportedDispatch {
		self.reported().await
	}
}
impl RouteDispatch {
	async fn reported(self: Box<Self>) -> ReportedDispatch {
		let mut provider_report = AttemptProviderReport::Unknown;
		let result = async {
			// The call credential is obtained immediately before I/O: a broker
			// Capability Token lives 60 s, and reserving the attempt may wait on
			// remote owners. A failure here, after the check, leaves the reserved
			// attempt charged, as for compaction.
			let access = self
				.access
				.resolve(&self.scope, Provider::Openrouter.base_url(), &self.source)
				.await
				.map_err(|_| Error::External("decision provider credential unavailable".into()))?;
			let mut request = self
				.client
				.post(route_url(&access.endpoint, &self.route.provider_contract)?)
				.header(reqwest::header::CONTENT_TYPE, "application/json")
				.body(self.body);
			if !access.bearer.expose_secret().is_empty() {
				request = request.bearer_auth(access.bearer.expose_secret());
			}
			let mut response = request
				.send()
				.await
				.map_err(|_| Error::External("decision provider transport failed".into()))?;
			let status = response.status().as_u16();
			// Read bounded, safe metadata on unsuccessful responses too. The original
			// HTTP failure class still wins over a malformed/oversized error body.
			let mut response_failure = if matches!(status, 400 | 401 | 402 | 403 | 413) {
				Some(DispatchError::Refused(status))
			} else if !response.status().is_success() {
				Some(DispatchError::ProviderFailure(Error::External(format!(
					"decision provider returned HTTP {status}"
				))))
			} else {
				None
			};
			// Bound the Node's transport buffer, including ignored provider metadata.
			// Current large valid responses still fit; the limit is checked before
			// appending each chunk and no response body is retained in evidence.
			const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
			if response
				.content_length()
				.is_some_and(|bytes| bytes > MAX_RESPONSE_BYTES as u64)
			{
				return Err(response_failure
					.take()
					.unwrap_or(DispatchError::InvalidAnswers));
			}
			let mut body = Vec::new();
			while let Some(chunk) = response.chunk().await.map_err(|_| {
				response_failure.take().unwrap_or_else(|| {
					DispatchError::ProviderFailure(Error::External(
						"decision response unavailable".into(),
					))
				})
			})? {
				if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
					return Err(response_failure
						.take()
						.unwrap_or(DispatchError::InvalidAnswers));
				}
				body.extend_from_slice(&chunk);
			}
			provider_report = reported_facts(&self.route, &body);
			if let Some(failure) = response_failure {
				return Err(failure);
			}
			let answered = validate_response(&self.route, &self.questions, &body)
				.map_err(|_| DispatchError::InvalidAnswers)?;
			// Safe telemetry remains useful; the same provider facts now reach durable evidence.
			tracing::info!(
				decision_attempt = %self.attempt,
				served_model = answered.served.as_str(),
				served_host = answered.host.as_deref().unwrap_or_default(),
				input_tokens = answered.input_tokens,
				cost_usd = answered.cost_usd,
				"Decision Attempt answered"
			);
			Ok(answered.answers)
		}
		.await;
		ReportedDispatch {
			result,
			provider_report,
		}
	}
}

/// System One lives under the Provider Catalog base (`…/api/v1`); the
/// Decisions API is the alpha endpoint beside it (`…/api/alpha/decisions`).
fn route_url(endpoint: &str, contract: &str) -> Result<String> {
	let base = endpoint.trim_end_matches('/');
	if contract == DECISIONS {
		let root = base.strip_suffix("/v1").ok_or_else(|| {
			Error::Invalid("decision endpoint is not an OpenRouter API base".into())
		})?;
		return Ok(format!("{root}/alpha/decisions"));
	}
	Ok(format!("{base}/systemone"))
}

/// Whether OpenRouter's reported host is one the Route names. OpenRouter
/// reports a display name (`DeepInfra`) where the Route names a slug
/// (`deepinfra`); both are compared with letters and digits only.
fn named_host(hosts: &[String], reported: &str) -> bool {
	let normalize = |name: &str| {
		name.chars()
			.filter(char::is_ascii_alphanumeric)
			.map(|c| c.to_ascii_lowercase())
			.collect::<String>()
	};
	let reported = normalize(reported);
	!reported.is_empty() && hosts.iter().any(|host| normalize(host) == reported)
}

/// The actual host is a verified configured slug, never an unchecked display string.
fn reported_host(route: &DeciderRoute, reported: &str) -> Option<String> {
	if route.provider_contract == DECISIONS {
		route
			.hosts
			.iter()
			.find(|host| named_host(std::slice::from_ref(host), reported))
			.cloned()
	} else if named_host(&["typesafe".to_owned()], reported) {
		Some("typesafe".into())
	} else {
		None
	}
}
fn reported_usage(
	raw: Option<&serde_json::value::RawValue>,
) -> (Option<u64>, Option<u64>, Option<ProviderCost>) {
	#[derive(serde::Deserialize)]
	struct Usage {
		input_tokens: Option<Box<serde_json::value::RawValue>>,
		output_tokens: Option<Box<serde_json::value::RawValue>>,
		cost: Option<Box<serde_json::value::RawValue>>,
	}
	let Some(usage) = raw.and_then(|raw| serde_json::from_str::<Usage>(raw.get()).ok()) else {
		return (None, None, None);
	};
	let tokens = |value: Option<Box<serde_json::value::RawValue>>| {
		value.and_then(|raw| serde_json::from_str::<u64>(raw.get()).ok())
	};
	let cost = usage
		.cost
		.and_then(|raw| raw.get().parse::<f64>().ok())
		.and_then(|value| ProviderCost::new(value).ok());
	(
		tokens(usage.input_tokens),
		tokens(usage.output_tokens),
		cost,
	)
}
fn reported_facts(route: &DeciderRoute, body: &[u8]) -> AttemptProviderReport {
	#[derive(serde::Deserialize)]
	struct Report {
		model: Option<Box<serde_json::value::RawValue>>,
		provider: Option<Box<serde_json::value::RawValue>>,
		usage: Option<Box<serde_json::value::RawValue>>,
	}
	let Ok(value) = serde_json::from_slice::<Report>(body) else {
		return AttemptProviderReport::Unknown;
	};
	let served_model = value
		.model
		.and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
		.filter(|model| served_by(&route.model, model));
	let served_host = value
		.provider
		.and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
		.and_then(|host| reported_host(route, &host));
	let (input_tokens, output_tokens, cost_usd) = reported_usage(value.usage.as_deref());
	if served_model.is_none()
		&& served_host.is_none()
		&& input_tokens.is_none()
		&& output_tokens.is_none()
		&& cost_usd.is_none()
	{
		AttemptProviderReport::Unknown
	} else {
		AttemptProviderReport::Reported {
			served_model,
			served_host,
			input_tokens,
			output_tokens,
			cost_usd,
		}
	}
}

/// Validated answers and the numeric usage the provider reported with them.
#[derive(Debug)]
struct Answered {
	answers: BTreeMap<String, Probability>,
	served: String,
	host: Option<String>,
	input_tokens: Option<u64>,
	cost_usd: Option<f64>,
}

fn validate_response(route: &DeciderRoute, questions: &Questions, body: &[u8]) -> Result<Answered> {
	#[derive(serde::Deserialize)]
	struct Response {
		model: Option<String>,
		provider: Option<String>,
		answers: Option<AnswerMap>,
		// Read leniently: malformed usage never rejects valid answers.
		usage: Option<Box<serde_json::value::RawValue>>,
	}
	#[derive(serde::Deserialize)]
	struct Answer {
		#[serde(rename = "type")]
		kind: Option<String>,
		noul: Option<Box<serde_json::value::RawValue>>,
	}
	struct AnswerMap(BTreeMap<String, Answer>);
	impl<'de> serde::Deserialize<'de> for AnswerMap {
		fn deserialize<D: serde::Deserializer<'de>>(
			deserializer: D,
		) -> std::result::Result<Self, D::Error> {
			struct Visitor;
			impl<'de> serde::de::Visitor<'de> for Visitor {
				type Value = AnswerMap;
				fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
					f.write_str("a map of unique question answers")
				}
				fn visit_map<M: serde::de::MapAccess<'de>>(
					self,
					mut map: M,
				) -> std::result::Result<AnswerMap, M::Error> {
					let mut answers = BTreeMap::new();
					while let Some((id, answer)) = map.next_entry::<String, Answer>()? {
						if answers.insert(id, answer).is_some() {
							return Err(serde::de::Error::custom("duplicate answer identity"));
						}
					}
					Ok(AnswerMap(answers))
				}
			}
			deserializer.deserialize_map(Visitor)
		}
	}
	let response: Response = serde_json::from_slice(body)
		.map_err(|_| Error::External("invalid decision response JSON".into()))?;
	let served = response
		.model
		.filter(|served| served_by(&route.model, served))
		.ok_or_else(|| {
			Error::External("decision response model differs from the pinned version".into())
		})?;
	// A Decisions Route must have been answered by a host it names.
	if route.provider_contract == DECISIONS
		&& !response
			.provider
			.as_deref()
			.is_some_and(|host| named_host(&route.hosts, host))
	{
		return Err(Error::External(
			"decision response came from a host the Route does not name".into(),
		));
	}
	let answers = response
		.answers
		.ok_or_else(|| Error::External("decision response has no answer map".into()))?
		.0;
	if answers.len() != questions.len() || questions.keys().any(|key| !answers.contains_key(key)) {
		return Err(Error::External(
			"decision response question coverage differs".into(),
		));
	}
	let mut validated = BTreeMap::new();
	for (id, answer) in answers {
		if answer.kind.as_deref().is_some_and(|kind| kind != "noul") {
			return Err(Error::External(
				"decision response contains an unsupported answer type".into(),
			));
		}
		// Parse the original decimal with Rust's binary64 parser. This preserves
		// adjacent threshold values without changing JSON parsing in other crates.
		let value = answer
			.noul
			.as_ref()
			.and_then(|raw| raw.get().parse::<f64>().ok())
			.ok_or_else(|| Error::External("decision response lacks a Noul probability".into()))?;
		validated.insert(
			id,
			Probability::new(value)
				.map_err(|_| Error::External("invalid decision probability".into()))?,
		);
	}
	validate_answers(questions, &validated)?;
	let (input_tokens, _, cost_usd) = reported_usage(response.usage.as_deref());
	Ok(Answered {
		answers: validated,
		served,
		host: response.provider,
		input_tokens,
		cost_usd: cost_usd.map(ProviderCost::value),
	})
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "decision/observation_scope_tests.rs"]
mod observation_scope_tests;
