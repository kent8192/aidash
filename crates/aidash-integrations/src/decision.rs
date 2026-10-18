//! Explicit Jev adapter through OpenRouter's System One API and the Tenant's
//! Provider Credential (ADR 0041); no environment-selected fallback. Requests
//! stay within the byte budget the pinned Decider version declares.
use crate::{Error, Result};
use aidash_application::{
	ports::decision::{DecisionProvider, DispatchError, PreparedDispatch, PreparedRequest},
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
pub trait JevCapacity: Send + Sync {
	fn fits(
		&self,
		model: &str,
		state: &Value,
		questions: &Questions,
		encoded: &[u8],
	) -> Result<bool>;
}

/// Jev's capacity as a Decider version declares it: a request may be as large
/// as the mode its state travels in allows (ADR 0042, 2026-10-17).
pub struct ByteBudget(pub StateBudget);
impl JevCapacity for ByteBudget {
	fn fits(&self, _: &str, state: &Value, _: &Questions, encoded: &[u8]) -> Result<bool> {
		let state = serde_json::to_vec(state)?.len();
		Ok(self
			.0
			.request_limit(state)
			.is_some_and(|limit| encoded.len() <= limit))
	}
}

pub struct JevDecisionProvider {
	config: DeciderConfig,
	implementation: String,
	client: reqwest::Client,
	access: Arc<dyn ProviderAccess>,
	/// The Run and its Tenant; each dispatch adds its own Decision Attempt scope.
	context: Context,
	capacity: Arc<dyn JevCapacity>,
}
impl JevDecisionProvider {
	/// The trusted Node catalog supplies this concrete implementation's opaque identity,
	/// independently of the publisher's configuration or a retained Run's requested pin.
	pub fn new(
		config: DeciderConfig,
		implementation: String,
		access: Arc<dyn ProviderAccess>,
		context: Context,
		capacity: Arc<dyn JevCapacity>,
		timeout: Duration,
	) -> Result<Self> {
		config.validate()?;
		if context.tenant.is_empty()
			|| context.run.is_none_or(|run| run.is_nil())
			|| context.maintenance.is_some()
		{
			return Err(Error::Invalid(
				"Decider calls are provider calls of one Run in its Tenant".into(),
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
	fn encode(&self, state: &Value, questions: &Questions) -> Result<PreparedRequest> {
		let questions_json: BTreeMap<_, _> = questions
			.iter()
			.map(|(key, q)| (key, json!({"type":"noul", "instructions":q.description})))
			.collect();
		Ok(PreparedRequest {
			// Zero data retention is required on every OpenRouter call.
			body: serde_json::to_vec(&json!({
				"model":self.config.model,"state":state,"questions":questions_json,
				"provider":{"zdr":true}
			}))?,
			questions: questions.clone(),
		})
	}
}
#[async_trait]
impl DecisionProvider for JevDecisionProvider {
	fn implementation_id(&self) -> &str {
		&self.implementation
	}
	fn configuration_digest(&self) -> Result<String> {
		Ok(self.config.digest()?)
	}
	fn plan(&self, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
		if questions.is_empty() || questions.values().any(|q| q.description.trim().is_empty()) {
			return Err(Error::Invalid(
				"decision requires complete described Noul questions".into(),
			));
		}
		let complete = self.encode(state, questions)?;
		if self
			.capacity
			.fits(&self.config.model, state, questions, &complete.body)?
		{
			return Ok(vec![complete]);
		}
		let mut batches = vec![];
		let mut pending = Questions::new();
		let mut last = None;
		for (id, question) in questions {
			pending.insert(id.clone(), question.clone());
			let request = self.encode(state, &pending)?;
			if self
				.capacity
				.fits(&self.config.model, state, &pending, &request.body)?
			{
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
			let request = self.encode(state, &pending)?;
			if !self
				.capacity
				.fits(&self.config.model, state, &pending, &request.body)?
			{
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
		request: &PreparedRequest,
		attempt: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		let body: Value = serde_json::from_slice(&request.body)
			.map_err(|_| Error::Invalid("invalid prepared decision payload".into()))?;
		let state = body
			.get("state")
			.ok_or_else(|| Error::Invalid("prepared decision has no state".into()))?;
		if request.questions.is_empty()
			|| self.encode(state, &request.questions)?.body != request.body
		{
			return Err(Error::Invalid(
				"prepared decision payload changed its pinned model or questions".into(),
			));
		}
		let mut scope = self.context.clone();
		scope.provider_credential_id = None;
		scope.inference = Some(Inference {
			model: self.config.model.clone(),
			operations: vec![Operation::Decision],
			max_output_tokens: 0,
			cache_breakpoints: false,
			call: Some(attempt),
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
		Ok(Box::new(JevDispatch {
			client: self.client.clone(),
			access: self.access.clone(),
			scope,
			source,
			body: request.body.clone(),
			attempt,
			model: self.config.model.clone(),
			questions: request.questions.clone(),
		}))
	}
}

struct JevDispatch {
	client: reqwest::Client,
	access: Arc<dyn ProviderAccess>,
	/// The Decision Attempt's call scope and the Tenant binding it uses.
	scope: Context,
	source: Source,
	body: Vec<u8>,
	/// The Decision Attempt this request is; a direct (self-hosted) call has no
	/// broker audit, so the response is traced under it.
	attempt: Uuid,
	model: String,
	questions: Questions,
}
#[async_trait]
impl PreparedDispatch for JevDispatch {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
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
			.post(format!(
				"{}/systemone",
				access.endpoint.trim_end_matches('/')
			))
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.body(self.body);
		if !access.bearer.expose_secret().is_empty() {
			request = request.bearer_auth(access.bearer.expose_secret());
		}
		let response = request
			.send()
			.await
			.map_err(|_| Error::External("decision provider transport failed".into()))?;
		if !response.status().is_success() {
			return Err(Error::External(format!(
				"decision provider returned HTTP {}",
				response.status().as_u16()
			))
			.into());
		}
		// serde_json recursion limits protect parsing structure, not payload size.
		// There is deliberately no fixed response byte limit or retained raw body.
		let body = response
			.bytes()
			.await
			.map_err(|_| Error::External("decision response unavailable".into()))?;
		let answered = validate_response(&self.model, &self.questions, &body)
			.map_err(|_| DispatchError::InvalidAnswers)?;
		// Until Decision Evidence keeps it (#108), cost is observable only here.
		tracing::info!(
			decision_attempt = %self.attempt,
			served_model = answered.served.as_str(),
			input_tokens = answered.input_tokens,
			cost_usd = answered.cost_usd,
			"Jev Decision Attempt answered"
		);
		Ok(answered.answers)
	}
}

/// Validated answers and the numeric usage the provider reported with them.
#[derive(Debug)]
struct Answered {
	answers: BTreeMap<String, Probability>,
	served: String,
	input_tokens: Option<u64>,
	cost_usd: Option<f64>,
}

fn validate_response(model: &str, questions: &Questions, body: &[u8]) -> Result<Answered> {
	#[derive(serde::Deserialize)]
	struct Response {
		model: Option<String>,
		answers: Option<AnswerMap>,
		// Read leniently: malformed usage never rejects valid answers.
		usage: Option<Value>,
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
		.filter(|served| served_by(model, served))
		.ok_or_else(|| {
			Error::External("decision response model differs from the pinned version".into())
		})?;
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
	let usage = response.usage.unwrap_or_default();
	Ok(Answered {
		answers: validated,
		served,
		input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
		cost_usd: usage.get("cost").and_then(Value::as_f64),
	})
}

#[cfg(test)]
mod tests;
