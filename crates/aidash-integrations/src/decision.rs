//! Explicit Jev adapter; no environment-selected fallback or Aidash payload ceilings.
use crate::{Error, Result};
use aidash_application::ports::{
	Credentials,
	decision::{DecisionProvider, DispatchError, PreparedRequest},
};
use aidash_domain::decision::*;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

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

pub struct JevDecisionProvider {
	config: DeciderConfig,
	client: reqwest::Client,
	credentials: Arc<dyn Credentials>,
	capacity: Arc<dyn JevCapacity>,
}
impl JevDecisionProvider {
	pub fn new(
		config: DeciderConfig,
		credentials: Arc<dyn Credentials>,
		capacity: Arc<dyn JevCapacity>,
		timeout: Duration,
	) -> Result<Self> {
		config.validate()?;
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
			client,
			credentials,
			capacity,
		})
	}
	fn encode(&self, state: &Value, questions: &Questions) -> Result<PreparedRequest> {
		let questions_json: BTreeMap<_, _> = questions
			.iter()
			.map(|(key, q)| (key, json!({"type":"noul", "instructions":q.description})))
			.collect();
		Ok(PreparedRequest {
			body: serde_json::to_vec(
				&json!({"model":self.config.model,"state":state,"questions":questions_json}),
			)?,
			questions: questions.clone(),
		})
	}
	fn key(&self) -> Result<reqwest::header::HeaderValue> {
		let secret = self
			.credentials
			.resolve(&self.config.credential_env)
			.map_err(|_| Error::Invalid("decision credential unavailable".into()))?;
		if secret.trim().is_empty() {
			return Err(Error::Invalid("decision credential is empty".into()));
		}
		let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {secret}"))
			.map_err(|_| Error::Invalid("invalid decision credential".into()))?;
		header.set_sensitive(true);
		Ok(header)
	}
	fn request(&self, prepared: &PreparedRequest) -> Result<reqwest::RequestBuilder> {
		Ok(self
			.client
			.post(&self.config.endpoint)
			.header(reqwest::header::AUTHORIZATION, self.key()?)
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.body(prepared.body.clone()))
	}
}
#[async_trait]
impl DecisionProvider for JevDecisionProvider {
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
	fn preflight(&self, request: &PreparedRequest) -> Result<()> {
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
		self.request(request)?
			.build()
			.map_err(|_| Error::Invalid("invalid decision HTTP request".into()))?;
		Ok(())
	}
	async fn dispatch(
		&self,
		request: &PreparedRequest,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
		self.preflight(request)?;
		let response = self
			.request(request)?
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
		validate_response(&self.config.model, &request.questions, &body)
			.map_err(|_| DispatchError::InvalidAnswers)
	}
}

fn validate_response(
	model: &str,
	questions: &Questions,
	body: &[u8],
) -> Result<BTreeMap<String, Probability>> {
	#[derive(serde::Deserialize)]
	struct Response {
		model: Option<String>,
		answers: Option<AnswerMap>,
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
	if response.model.as_deref() != Some(model) {
		return Err(Error::External(
			"decision response model differs from the pinned version".into(),
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
	Ok(validated)
}

#[cfg(test)]
mod tests;
