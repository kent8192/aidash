//! Host-owned research use cases. One implementation serves every provider
//! package: it validates input, enforces Run limits, waits for Web disclosure
//! approval, attributes cost, records Research evidence and bounds results.
use crate::{
	Error, Result,
	ports::research::{
		Budget, DisclosureGate, DisclosureState, EvidenceStore, Outbound, PageProvider,
		PageRenderer, Reservation, Reserved, RetrievalTerms, SearchProvider, WebDisclosure,
	},
};
use aidash_domain::research::{
	Failure, MAX_RESULT_BYTES, Operation,
	account::{AccountTerms, CostAttribution, month_start},
	document::{
		self, Completeness, Cursor, DEFAULT_DELIVERY_BYTES, MAX_DELIVERY_BYTES, MAX_FIND_MATCHES,
		MAX_FIND_QUERY_CHARS, Passage, Position,
	},
	envelope,
	evidence::{AttemptOutcome, AttemptState, DocumentSnapshot, Limits},
	failure,
	search::{SearchQuery, candidate_host},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

pub mod reading;

/// Instruction returned with a pending Web disclosure approval.
pub const APPROVAL_INSTRUCTION: &str = "Web disclosure approval is required before this request leaves the Node. The Run waits for the decision; after approval, repeat this exact call.";
/// Instruction returned with the pages of a document that carry no text.
pub const VIEW_INSTRUCTION: &str = "These pages carry no text. View one as an image with the view operation, if it is bound; a model without image input cannot read them.";
/// Instruction returned when those pages cannot be viewed.
pub const NOT_RETAINED_INSTRUCTION: &str =
	"These pages carry no text, and the document is not retained, so they cannot be viewed.";

/// The Agent-facing definition of a research operation, identical for every
/// provider package so that providers can replace each other.
pub fn specification(operation: Operation) -> aidash_domain::provider::ToolSpec {
	let (description, parameters) = match operation {
		Operation::Search => (
			"Web search for public sources. Results are Source records to read, not evidence: snippets cannot be cited. Read a source with the fetch operation before citing it. If the result is approval_required, the Run waits for Web disclosure approval; then repeat the identical call.",
			aidash_domain::research::search::search_schema(),
		),
		Operation::Fetch => (
			"Read one public Web page as a fixed Document snapshot. Give a source_id from search or an http(s) url; continue a snapshot with document_id and next_cursor. Each delivered passage has an evidence_ref; cite it in the final answer as [[cite:<evidence_ref>]]. If the result is approval_required, the Run waits for Web disclosure approval; then repeat the identical call.",
			json!({
				"type": "object",
				"properties": {
					"source_id": {"type": "string", "format": "uuid"},
					"url": {"type": "string", "maxLength": aidash_domain::research::search::MAX_URL_BYTES},
					"document_id": {"type": "string", "format": "uuid"},
					"cursor": {"type": "string", "maxLength": 1100},
					"max_bytes": {"type": "integer", "minimum": 1, "maximum": MAX_DELIVERY_BYTES},
				},
				"additionalProperties": false,
			}),
		),
		Operation::Find => (
			"Find lines containing a query in a Document snapshot already read in this Run. Matching is case-insensitive unless case_sensitive is true. Each match has an evidence_ref to cite as [[cite:<evidence_ref>]]. Nothing leaves the Node.",
			json!({
				"type": "object",
				"required": ["document_id", "query"],
				"properties": {
					"document_id": {"type": "string", "format": "uuid"},
					"query": {"type": "string", "minLength": 1, "maxLength": MAX_FIND_QUERY_CHARS},
					"case_sensitive": {"type": "boolean"},
					"cursor": {"type": "string", "maxLength": 1100},
					"max_matches": {"type": "integer", "minimum": 1, "maximum": MAX_FIND_MATCHES},
				},
				"additionalProperties": false,
			}),
		),
		Operation::View => (
			"View one page of a document already read in this Run as an image, when its fetch result lists the page under pages_without_text. Give document_id and page. The image is an Evidence fragment with an evidence_ref; cite it as [[cite:<evidence_ref>]]. Nothing leaves the Node.",
			json!({
				"type": "object",
				"required": ["document_id", "page"],
				"properties": {
					"document_id": {"type": "string", "format": "uuid"},
					"page": {"type": "integer", "minimum": 1},
				},
				"additionalProperties": false,
			}),
		),
	};
	aidash_domain::provider::ToolSpec {
		name: aidash_domain::tool::providers::research_alias(operation).into(),
		description: description.into(),
		parameters,
	}
}

/// One research invocation of a bound operation.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
	pub run: Uuid,
	/// The Run's observed input sequence at dispatch.
	pub context_revision: i64,
	/// The Harness invocation key; a repeated key never dispatches again.
	pub key: &'a str,
	/// The bound Node provider, e.g. `research.brave@1`.
	pub provider: &'a str,
	/// Declared by the operation's contract (`disclosure_approval`).
	pub disclosure_approval: bool,
}

/// One request's charge to a Research account, checked again under the
/// reservation's lock.
struct Charge {
	attribution: CostAttribution,
	budget: Budget,
	/// The terms a Provider result is retrieved under (ADR 0058).
	retention: RetrievalTerms,
}

/// What an approver is shown besides the exact outbound content.
#[derive(Debug, Default)]
struct Card {
	attribution: Option<CostAttribution>,
	query: Option<Value>,
}

pub struct Research<'a> {
	pub store: &'a dyn EvidenceStore,
	pub gate: &'a dyn DisclosureGate,
	pub limits: Limits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchInput {
	source_id: Option<Uuid>,
	url: Option<String>,
	document_id: Option<Uuid>,
	cursor: Option<String>,
	max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewInput {
	document_id: Uuid,
	page: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindInput {
	document_id: Uuid,
	query: String,
	#[serde(default)]
	case_sensitive: bool,
	cursor: Option<String>,
	max_matches: Option<usize>,
}

impl Research<'_> {
	pub async fn search(
		&self,
		call: &Call<'_>,
		provider: &dyn SearchProvider,
		input: &Value,
	) -> Result<Value> {
		let operation = Operation::Search;
		if let Some(revoked) = self.revoked(call, operation).await? {
			return Ok(revoked);
		}
		let Ok(query) = SearchQuery::parse(input, None) else {
			return Ok(failure(operation, Failure::InvalidInput));
		};
		if let Some(previous) = self.replay(call, operation).await? {
			return Ok(previous);
		}
		let usage = self.store.usage(call.run).await?;
		if usage.search_attempts >= self.limits.search_attempts {
			return Ok(failure(operation, Failure::AttemptLimit));
		}
		if usage.sources >= self.limits.sources {
			return Ok(failure(operation, Failure::SourceLimit));
		}
		let charge = match self.charge(operation, provider.account()).await? {
			Ok(charge) => charge,
			Err(blocked) => return Ok(blocked),
		};
		let outbound = match provider.disclosure(&query) {
			Ok(outbound) => outbound,
			Err(error) => return Ok(failure(operation, error)),
		};
		let card = Card {
			attribution: charge.as_ref().map(|charge| charge.attribution.clone()),
			query: Some(query.effective()),
		};
		let approval = match self.disclose(call, operation, &outbound, card).await? {
			Ok(approval) => approval,
			Err(blocked) => return Ok(blocked),
		};
		if let Err(blocked) = self
			.reserve(
				call,
				Reservation {
					key: call.key.into(),
					operation,
					provider: call.provider.into(),
					destination: outbound.destination,
					attribution: charge.as_ref().map(|charge| charge.attribution.clone()),
					retention: charge.as_ref().map(|charge| charge.retention.clone()),
					limits: self.limits,
					budget: charge.map(|charge| charge.budget),
					source_url: None,
				},
			)
			.await?
		{
			return Ok(blocked);
		}
		if let Some(blocked) = self.consume(call, operation, approval).await? {
			return Ok(blocked);
		}
		let searched_at = Utc::now();
		let results = match provider.search(&query).await {
			Ok(results) => results,
			Err(error) => return self.settle_failure(call, operation, error, true).await,
		};
		let mut page = query.normalize(results);
		// The store keeps the Run within its Source record limit under its lock.
		let records = match self
			.store
			.record_sources(call.run, call.key, &page.sources, self.limits.sources)
			.await?
		{
			Ok(records) => records,
			// Revoked while the provider answered: nothing is recorded, and the
			// attempt stays charged.
			Err(refused) => {
				let result = failure(operation, refused);
				return self
					.store
					.settle(call.run, call.key, AttemptOutcome::Completed, &result)
					.await;
			}
		};
		if records.len() < page.sources.len() {
			page.sources.truncate(records.len());
			page.truncated = true;
		}
		let mut sources = page
			.sources
			.iter()
			.zip(&records)
			.map(|(source, record)| {
				json!({
					"source_id": record.id,
					"url": source.url,
					"title": source.title,
					"snippet": source.snippet,
					"rank": source.rank,
					"page_age": source.page_age,
					"evidence_state": "unread",
					"truncated": source.truncated,
				})
			})
			.collect::<Vec<_>>();
		let mut truncated = page.truncated;
		let result = loop {
			let data = json!({
				"provider": call.provider,
				"searched_at": searched_at,
				"effective": query.effective(),
				"returned_count": sources.len(),
				"filtered_count": page.filtered_count,
				"more_results_available": page.more_results_available,
				"sources": sources,
			});
			let status = if sources.is_empty() { "empty" } else { "ok" };
			let result = envelope(operation, status, data, truncated);
			// Recorded Source records stay valid; only their delivery is bounded.
			if result.to_string().len() <= MAX_RESULT_BYTES || sources.pop().is_none() {
				break result;
			}
			truncated = true;
		};
		let result = if result.to_string().len() > MAX_RESULT_BYTES {
			failure(operation, Failure::ResponseTooLarge)
		} else {
			result
		};
		self.store
			.settle(call.run, call.key, AttemptOutcome::Completed, &result)
			.await
	}

	pub async fn fetch(
		&self,
		call: &Call<'_>,
		provider: &dyn PageProvider,
		input: &Value,
	) -> Result<Value> {
		let operation = Operation::Fetch;
		if let Some(revoked) = self.revoked(call, operation).await? {
			return Ok(revoked);
		}
		let Ok(input) = serde_json::from_value::<FetchInput>(input.clone()) else {
			return Ok(failure(operation, Failure::InvalidInput));
		};
		let max_bytes = input.max_bytes.unwrap_or(DEFAULT_DELIVERY_BYTES);
		let targets = [
			input.source_id.is_some(),
			input.url.is_some(),
			input.document_id.is_some(),
		];
		if targets.iter().filter(|target| **target).count() != 1
			|| (input.cursor.is_some() && input.document_id.is_none())
			|| !(1..=MAX_DELIVERY_BYTES).contains(&max_bytes)
		{
			return Ok(failure(operation, Failure::InvalidInput));
		}
		if let Some(document) = input.document_id {
			// Continued reading of a retained snapshot discloses nothing.
			let Some(snapshot) = self.store.snapshot(call.run, document).await? else {
				return Ok(failure(operation, Failure::UnknownDocument));
			};
			if let Err(purged) = snapshot.readable() {
				return Ok(failure(operation, purged));
			}
			let position = match input.cursor.as_deref().map(|cursor| {
				Cursor::decode(cursor)?.check(snapshot.id, &snapshot.text_digest, operation, None)
			}) {
				None => Position::default(),
				Some(Ok(position)) => position,
				Some(Err(_)) => return Ok(failure(operation, Failure::InvalidCursor)),
			};
			return self.deliver(call, &snapshot, position, max_bytes).await;
		}
		if let Some(previous) = self.replay(call, operation).await? {
			return Ok(previous);
		}
		let (source, address) = match (input.source_id, input.url) {
			(Some(id), _) => match self.store.source(call.run, id).await? {
				Some(source) => (Some(source.id), source.url),
				None => return Ok(failure(operation, Failure::UnknownSource)),
			},
			(None, Some(url)) => (None, url),
			(None, None) => return Ok(failure(operation, Failure::InvalidInput)),
		};
		let (Some(host), Ok(url)) = (candidate_host(&address), Url::parse(&address)) else {
			return Ok(failure(operation, Failure::InvalidInput));
		};
		if let Err(error) = provider.admit(&url) {
			return Ok(failure(operation, error));
		}
		let usage = self.store.usage(call.run).await?;
		if usage.fetch_attempts >= self.limits.fetch_attempts {
			return Ok(failure(operation, Failure::AttemptLimit));
		}
		if usage.snapshots >= self.limits.snapshots
			|| usage.snapshot_bytes >= self.limits.snapshot_bytes
		{
			return Ok(failure(operation, Failure::SnapshotLimit));
		}
		// A page fetched by address becomes a new Source record.
		if source.is_none() && usage.sources >= self.limits.sources {
			return Ok(failure(operation, Failure::SourceLimit));
		}
		let charge = match self.charge(operation, provider.account()).await? {
			Ok(charge) => charge,
			Err(blocked) => return Ok(blocked),
		};
		let outbound = Outbound {
			destination: host,
			content: url.to_string(),
		};
		let card = Card {
			attribution: charge.as_ref().map(|charge| charge.attribution.clone()),
			query: None,
		};
		let approval = match self.disclose(call, operation, &outbound, card).await? {
			Ok(approval) => approval,
			Err(blocked) => return Ok(blocked),
		};
		// The reservation creates the Source record of a page read by address,
		// so a concurrent invocation cannot take its slot before dispatch.
		let created = match self
			.reserve(
				call,
				Reservation {
					key: call.key.into(),
					operation,
					provider: call.provider.into(),
					destination: outbound.destination,
					attribution: charge.as_ref().map(|charge| charge.attribution.clone()),
					retention: charge.as_ref().map(|charge| charge.retention.clone()),
					limits: self.limits,
					budget: charge.map(|charge| charge.budget),
					source_url: source.is_none().then(|| url.to_string()),
				},
			)
			.await?
		{
			Ok(created) => created,
			Err(blocked) => return Ok(blocked),
		};
		let Some(source) = source.or(created) else {
			return Err(crate::Error::Invalid(
				"the reservation created no Source record".into(),
			));
		};
		if let Some(blocked) = self.consume(call, operation, approval).await? {
			return Ok(blocked);
		}
		let page = match provider.fetch(&url).await {
			Ok(page) => page,
			Err(error) => return self.settle_failure(call, operation, error, true).await,
		};
		// The maximum age counts from `fetched_at`, so a provider must not
		// date a page after its own retrieval.
		if page.validate().is_err() || page.fetched_at > Utc::now() {
			return self
				.settle_failure(call, operation, Failure::ProviderResponseInvalid, true)
				.await;
		}
		// The store checks the snapshot limits under the Run's lock.
		let snapshot = match self
			.store
			.record_snapshot(
				call.run,
				call.key,
				source,
				url.as_str(),
				&page,
				&self.limits,
			)
			.await?
		{
			Ok(snapshot) => snapshot,
			Err(limit) => {
				let result = failure(operation, limit);
				return self
					.store
					.settle(call.run, call.key, AttemptOutcome::Completed, &result)
					.await;
			}
		};
		let result = self
			.deliver(call, &snapshot, Position::default(), max_bytes)
			.await?;
		self.store
			.settle(call.run, call.key, AttemptOutcome::Completed, &result)
			.await
	}

	/// Locate passages of a retained snapshot. Every match is delivered as an
	/// Evidence fragment; nothing leaves the Node.
	pub async fn find(&self, call: &Call<'_>, input: &Value) -> Result<Value> {
		let operation = Operation::Find;
		if let Some(revoked) = self.revoked(call, operation).await? {
			return Ok(revoked);
		}
		let Ok(input) = serde_json::from_value::<FindInput>(input.clone()) else {
			return Ok(failure(operation, Failure::InvalidInput));
		};
		let maximum = input.max_matches.unwrap_or(5);
		if input.query.trim().is_empty()
			|| input.query.chars().count() > MAX_FIND_QUERY_CHARS
			|| !(1..=MAX_FIND_MATCHES).contains(&maximum)
		{
			return Ok(failure(operation, Failure::InvalidInput));
		}
		let Some(snapshot) = self.store.snapshot(call.run, input.document_id).await? else {
			return Ok(failure(operation, Failure::UnknownDocument));
		};
		if let Err(purged) = snapshot.readable() {
			return Ok(failure(operation, purged));
		}
		let binding = document::find_binding(&input.query, input.case_sensitive);
		let start = match input.cursor.as_deref().map(|cursor| {
			Cursor::decode(cursor)?.check(
				snapshot.id,
				&snapshot.text_digest,
				operation,
				Some(&binding),
			)
		}) {
			None => 0,
			Some(Ok(position)) => position.line,
			Some(Err(_)) => return Ok(failure(operation, Failure::InvalidCursor)),
		};
		let (lines, mut next) = document::find(
			&snapshot.lines,
			&input.query,
			input.case_sensitive,
			start,
			maximum,
		);
		let mut matches = Vec::new();
		for index in lines {
			let line = &snapshot.lines[index];
			let passage = Passage {
				text: line.text.clone(),
				line_start: index + 1,
				line_end: index + 1,
				pages: line.page.into_iter().collect(),
			};
			let probe = find_result(
				&snapshot,
				&matches,
				Some(&placeholder(&snapshot, &passage)),
				Some(index),
			);
			if probe.to_string().len() > MAX_RESULT_BYTES {
				if matches.is_empty() {
					return Ok(failure(operation, Failure::ResponseTooLarge));
				}
				next = Some(index);
				break;
			}
			let fragment = match self
				.store
				.record_fragment(call.run, &snapshot, &passage)
				.await?
			{
				Ok(fragment) => fragment,
				Err(refused) => return Ok(failure(operation, refused)),
			};
			matches.push(fragment.delivered(&snapshot));
		}
		let cursor = next.map(|line| {
			Cursor {
				document: snapshot.id,
				digest: snapshot.text_digest.clone(),
				operation,
				position: Position { line, offset: 0 },
				binding: Some(binding.clone()),
			}
			.encode()
		});
		let mut result = find_result(&snapshot, &matches, None, None);
		result["data"]["next_cursor"] = json!(cursor);
		result["limits"]["truncated"] = json!(cursor.is_some());
		result["status"] = json!(if matches.is_empty() {
			"empty"
		} else if cursor.is_some() {
			"partial"
		} else {
			"ok"
		});
		Ok(result)
	}

	/// Render a page that carries no text from the retained original as a
	/// page-image Evidence fragment for the Run's model (ADR 0053). Nothing
	/// leaves the Node.
	pub async fn view(
		&self,
		call: &Call<'_>,
		renderer: &dyn PageRenderer,
		input: &Value,
	) -> Result<Value> {
		let operation = Operation::View;
		if let Some(revoked) = self.revoked(call, operation).await? {
			return Ok(revoked);
		}
		let Ok(input) = serde_json::from_value::<ViewInput>(input.clone()) else {
			return Ok(failure(operation, Failure::InvalidInput));
		};
		let Some(snapshot) = self.store.snapshot(call.run, input.document_id).await? else {
			return Ok(failure(operation, Failure::UnknownDocument));
		};
		if let Err(purged) = snapshot.readable() {
			return Ok(failure(operation, purged));
		}
		if !snapshot.pages_without_text.contains(&input.page) {
			return Ok(failure(operation, Failure::PageNotViewable));
		}
		if !snapshot
			.original
			.as_ref()
			.is_some_and(|original| original.retained)
		{
			return Ok(failure(operation, Failure::OriginalNotRetained));
		}
		// A page is one piece of evidence: viewing it again, or replaying a
		// view whose result was lost, returns the fragment already recorded
		// and takes no second view.
		if let Some(fragment) = self
			.store
			.viewed_page(call.run, snapshot.id, input.page)
			.await?
		{
			return Ok(envelope(
				operation,
				"ok",
				fragment.delivered(&snapshot),
				false,
			));
		}
		if self.store.usage(call.run).await?.page_views >= self.limits.page_views {
			return Ok(failure(operation, Failure::ViewLimit));
		}
		let Some(original) = self.store.original(call.run, snapshot.id).await? else {
			return Ok(failure(operation, Failure::OriginalNotRetained));
		};
		let image = match renderer.render(original, input.page).await {
			Ok(image) if image.validate().is_ok() => image,
			Ok(_) => return Ok(failure(operation, Failure::RenderFailed)),
			Err(error) => return Ok(failure(operation, error)),
		};
		// The store checks the view limit, revocation and purge again under the
		// Run's lock.
		let fragment = match self
			.store
			.record_page_image(call.run, &snapshot, input.page, &image, &self.limits)
			.await?
		{
			Ok(fragment) => fragment,
			Err(refused) => return Ok(failure(operation, refused)),
		};
		Ok(envelope(
			operation,
			"ok",
			fragment.delivered(&snapshot),
			false,
		))
	}

	/// Deliver one passage and retain it as an Evidence fragment. The passage
	/// shrinks until its JSON result fits; the fragment is recorded only then.
	async fn deliver(
		&self,
		call: &Call<'_>,
		snapshot: &DocumentSnapshot,
		position: Position,
		max_bytes: usize,
	) -> Result<Value> {
		let operation = Operation::Fetch;
		if snapshot.lines.is_empty() {
			// Every page of the document carries no text: nothing is delivered.
			return Ok(textless_result(snapshot));
		}
		let mut budget = max_bytes;
		let (delivery, fragment) = loop {
			let Ok(delivery) = document::deliver(&snapshot.lines, position, budget) else {
				return Ok(failure(operation, Failure::InvalidCursor));
			};
			let probe = fetch_result(
				snapshot,
				placeholder(snapshot, &delivery.passage),
				delivery.next,
			);
			if probe.to_string().len() <= MAX_RESULT_BYTES {
				let fragment = match self
					.store
					.record_fragment(call.run, snapshot, &delivery.passage)
					.await?
				{
					Ok(fragment) => fragment,
					Err(refused) => return Ok(failure(operation, refused)),
				};
				break (delivery, fragment);
			}
			if budget <= 1 {
				return Ok(failure(operation, Failure::ResponseTooLarge));
			}
			budget /= 2;
		};
		Ok(fetch_result(
			snapshot,
			fragment.delivered(snapshot),
			delivery.next,
		))
	}

	/// A Run whose evidence was revoked does no further research (ADR 0046).
	async fn revoked(&self, call: &Call<'_>, operation: Operation) -> Result<Option<Value>> {
		Ok(self
			.store
			.revoked(call.run)
			.await?
			.then(|| failure(operation, Failure::EvidenceRevoked)))
	}

	async fn replay(&self, call: &Call<'_>, operation: Operation) -> Result<Option<Value>> {
		Ok(match self.store.attempt(call.run, call.key).await? {
			None => None,
			// The step that reserved it stopped before settling it, so the
			// provider may have performed it. Settle it as unknown, so the
			// Run's usage reports it as such rather than as still in flight.
			Some(AttemptState::Dispatched) => {
				let result = failure(operation, Failure::OutcomeUnknown);
				match self
					.store
					.settle(call.run, call.key, AttemptOutcome::Unknown, &result)
					.await
				{
					// The tombstone instead when the Run was revoked meanwhile.
					Ok(recorded) => Some(recorded),
					// A late settlement or a concurrent replay came first:
					// answer with what it recorded.
					Err(Error::Conflict(_)) => {
						match self.store.attempt(call.run, call.key).await? {
							Some(AttemptState::Settled { result, .. }) => Some(result),
							_ => Some(result),
						}
					}
					Err(error) => return Err(error),
				}
			}
			Some(AttemptState::Settled { result, .. }) => Some(result),
		})
	}

	/// The charge of one `operation` request to the provider's Research
	/// account, if it has one, once the month's charges leave room for it.
	/// `Err` is the result to return instead of dispatching.
	async fn charge(
		&self,
		operation: Operation,
		account: std::result::Result<Option<AccountTerms>, Failure>,
	) -> Result<std::result::Result<Option<Charge>, Value>> {
		let account = match account {
			Ok(Some(account)) => account,
			Ok(None) => return Ok(Ok(None)),
			Err(error) => return Ok(Err(failure(operation, error))),
		};
		let Some(attribution) = account.attribution(operation) else {
			return Ok(Err(failure(operation, Failure::AccountUnavailable)));
		};
		let since = month_start(Utc::now());
		let spent = self.store.account_spend(&account.account_id, since).await?;
		if !account.admits(operation, spent) {
			return Ok(Err(failure(operation, Failure::BudgetExhausted)));
		}
		Ok(Ok(Some(Charge {
			attribution,
			budget: Budget {
				since,
				limit_micro_usd: account.request_allowance_micro_usd(),
			},
			retention: RetrievalTerms::of(&account),
		})))
	}

	/// `Err` is the result to return instead of dispatching; `Ok` carries a
	/// one-time approval to consume once the attempt is reserved.
	async fn disclose(
		&self,
		call: &Call<'_>,
		operation: Operation,
		outbound: &Outbound,
		card: Card,
	) -> Result<std::result::Result<Option<Uuid>, Value>> {
		if !call.disclosure_approval {
			return Ok(Ok(None));
		}
		let disclosure = WebDisclosure {
			operation,
			provider: call.provider.into(),
			outbound: outbound.clone(),
			context_revision: call.context_revision,
			attribution: card.attribution,
			query: card.query,
		};
		Ok(match self.gate.admit(call.run, &disclosure).await? {
			DisclosureState::Permitted => Ok(None),
			DisclosureState::Approved { approval_id } => Ok(Some(approval_id)),
			DisclosureState::Pending {
				approval_id,
				expires_at,
			} => Err(json!({
				// It carries the disclosed content, so a revocation redacts it
				// like every other research result.
				"contract": aidash_domain::research::RESULT_CONTRACT,
				"version": 1,
				"operation": operation.as_str(),
				"status": "approval_required",
				"approval_id": approval_id,
				"expires_at": expires_at,
				"instruction": APPROVAL_INSTRUCTION,
				"data": {
					"destination": outbound.destination,
					"content": outbound.content,
					"request_digest": disclosure.digest(call.run),
				},
			})),
			DisclosureState::Denied => Err(failure(operation, Failure::DisclosureDenied)),
			DisclosureState::Revoked => Err(failure(operation, Failure::EvidenceRevoked)),
		})
	}

	/// Consume a one-time approval after the reservation. A lost race settles
	/// the reserved attempt without sending anything.
	async fn consume(
		&self,
		call: &Call<'_>,
		operation: Operation,
		approval: Option<Uuid>,
	) -> Result<Option<Value>> {
		let Some(approval) = approval else {
			return Ok(None);
		};
		if self.gate.consume(call.run, approval).await? {
			return Ok(None);
		}
		self.settle_failure(call, operation, Failure::DisclosureDenied, false)
			.await
			.map(Some)
	}

	/// `Ok` carries the Source record the reservation created; `Err` is the
	/// result to return instead of dispatching.
	async fn reserve(
		&self,
		call: &Call<'_>,
		reservation: Reservation,
	) -> Result<std::result::Result<Option<Uuid>, Value>> {
		let operation = reservation.operation;
		Ok(match self.store.reserve(call.run, &reservation).await? {
			Reserved::Reserved { source } => Ok(source),
			// A concurrent invocation with the same key owns the attempt.
			Reserved::Duplicate => Err(failure(operation, Failure::OutcomeUnknown)),
			Reserved::AttemptLimit => Err(failure(operation, Failure::AttemptLimit)),
			Reserved::SourceLimit => Err(failure(operation, Failure::SourceLimit)),
			Reserved::SnapshotLimit => Err(failure(operation, Failure::SnapshotLimit)),
			Reserved::BudgetExhausted => Err(failure(operation, Failure::BudgetExhausted)),
			Reserved::Revoked => Err(failure(operation, Failure::EvidenceRevoked)),
		})
	}

	/// Settle a failed attempt. Once the provider was `called`, the request
	/// may have been sent and charged, so its outcome stays attributed.
	async fn settle_failure(
		&self,
		call: &Call<'_>,
		operation: Operation,
		error: Failure,
		called: bool,
	) -> Result<Value> {
		let result = failure(operation, error);
		let outcome = if !called {
			AttemptOutcome::Failed
		} else if error.uncertain() {
			AttemptOutcome::Unknown
		} else {
			AttemptOutcome::Completed
		};
		self.store
			.settle(call.run, call.key, outcome, &result)
			.await
	}
}

/// Delivered fragment JSON with a placeholder reference, for size probes.
fn placeholder(snapshot: &DocumentSnapshot, passage: &Passage) -> Value {
	aidash_domain::research::evidence::EvidenceFragment::new(
		aidash_domain::research::citations::EvidenceRef(Uuid::nil()),
		snapshot,
		passage,
	)
	.delivered(snapshot)
}

fn fetch_result(snapshot: &DocumentSnapshot, mut fragment: Value, next: Option<Position>) -> Value {
	let cursor = next.map(|position| {
		Cursor {
			document: snapshot.id,
			digest: snapshot.text_digest.clone(),
			operation: Operation::Fetch,
			position,
			binding: None,
		}
		.encode()
	});
	let partial = cursor.is_some() || snapshot.completeness != Completeness::Complete;
	fragment["requested_url"] = json!(snapshot.requested_url);
	fragment["media_type"] = json!(snapshot.media_type);
	fragment["next_cursor"] = json!(cursor);
	if let Some(count) = snapshot.page_count {
		fragment["page_count"] = json!(count);
	}
	if let Some(pages) = pages_without_text(snapshot) {
		fragment["pages_without_text"] = pages;
	}
	envelope(
		Operation::Fetch,
		if partial { "partial" } else { "ok" },
		fragment,
		partial,
	)
}

/// The pages of a snapshot that carry no text, and whether their images can be
/// viewed from the retained original (ADR 0053).
fn pages_without_text(snapshot: &DocumentSnapshot) -> Option<Value> {
	if snapshot.pages_without_text.is_empty() {
		return None;
	}
	let viewable = snapshot.purged.is_none()
		&& snapshot
			.original
			.as_ref()
			.is_some_and(|original| original.retained);
	Some(json!({
		"pages": snapshot.pages_without_text,
		"page_images": if viewable { "available" } else { "original_not_retained" },
		"instruction": if viewable { VIEW_INSTRUCTION } else { NOT_RETAINED_INSTRUCTION },
	}))
}

/// The result of reading a document whose pages all carry no text.
fn textless_result(snapshot: &DocumentSnapshot) -> Value {
	envelope(
		Operation::Fetch,
		"empty",
		json!({
			"source_id": snapshot.source_id,
			"document_id": snapshot.id,
			"url": snapshot.url,
			"requested_url": snapshot.requested_url,
			"title": snapshot.title,
			"media_type": snapshot.media_type,
			"fetched_at": snapshot.fetched_at,
			"text_digest": snapshot.text_digest,
			"completeness": snapshot.completeness.as_str(),
			"page_count": snapshot.page_count,
			"pages_without_text": pages_without_text(snapshot),
			"next_cursor": null,
		}),
		false,
	)
}

fn find_result(
	snapshot: &DocumentSnapshot,
	matches: &[Value],
	probe: Option<&Value>,
	probe_line: Option<usize>,
) -> Value {
	let mut matches = matches.to_vec();
	matches.extend(probe.cloned());
	let probe_cursor = probe_line.map(|line| {
		Cursor {
			document: snapshot.id,
			digest: snapshot.text_digest.clone(),
			operation: Operation::Find,
			position: Position { line, offset: 0 },
			binding: Some(String::from("sha256:") + &"0".repeat(64)),
		}
		.encode()
	});
	envelope(
		Operation::Find,
		"ok",
		json!({
			"document_id": snapshot.id,
			"matches": matches,
			"next_cursor": probe_cursor,
			"case_folding": document::CASE_FOLDING,
		}),
		probe_cursor.is_some(),
	)
}

pub mod account_terms;
pub mod retention;
pub mod revocation;

#[cfg(test)]
mod tests;
