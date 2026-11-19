//! Page reading (#987): one public Web page, read again on every call and
//! returned one bounded part at a time. Nothing is kept beyond the Run's
//! Context Journal, and each call needs a Tool call approval like a Tenant's
//! tool; a Run grant covers one host until the Run's next user input.
use crate::{Result, ports::research::PageProvider};
use aidash_domain::research::{
	Failure, MAX_RESULT_BYTES, Operation,
	address::{MAX_URL_BYTES, candidate_host},
	document::MAX_PART_BYTES,
	envelope, failure, parts,
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

/// The Agent-facing definition of page reading.
pub fn specification() -> aidash_domain::provider::ToolSpec {
	aidash_domain::provider::ToolSpec {
		name: aidash_domain::tool::providers::research_alias(Operation::Fetch).into(),
		description: "Read one public Web page as text. Give an http(s) url; a long page comes in parts, so read the next one with part. Every call reads the page again, so its text may change between parts. Pages of a PDF that carry no text are listed under pages_without_text. Each call needs a Tool call approval: the Run waits for the decision and then reads the page.".into(),
		parameters: json!({
			"type": "object",
			"required": ["url"],
			"properties": {
				"url": {"type": "string", "maxLength": MAX_URL_BYTES},
				"part": {"type": "integer", "minimum": 1},
			},
			"additionalProperties": false,
		}),
	}
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
	url: String,
	part: Option<usize>,
}

/// The host a page reading call would contact, which a Run grant is limited
/// to; `None` for input the call itself refuses.
pub fn page_host(input: &Value) -> Option<String> {
	let input = ReadInput::deserialize(input).ok()?;
	candidate_host(&input.url)
}

/// Read one part of the page `input` names through `provider`. Every refusal
/// is an ordinary result the model can act on; nothing is recorded.
pub async fn read(provider: &dyn PageProvider, input: &Value) -> Result<Value> {
	let operation = Operation::Fetch;
	let Ok(input) = ReadInput::deserialize(input) else {
		return Ok(failure(operation, Failure::InvalidInput));
	};
	let part = input.part.unwrap_or(1);
	let (Some(_), Ok(url)) = (candidate_host(&input.url), Url::parse(&input.url)) else {
		return Ok(failure(operation, Failure::InvalidInput));
	};
	if part == 0 {
		return Ok(failure(operation, Failure::InvalidInput));
	}
	if let Err(error) = provider.admit(&url) {
		return Ok(failure(operation, error));
	}
	let page = match provider.fetch(&url).await {
		Ok(page) => page,
		Err(error) => return Ok(failure(operation, error)),
	};
	if page.validate().is_err() {
		return Ok(failure(operation, Failure::ProviderResponseInvalid));
	}
	let text = page
		.lines
		.iter()
		.map(|line| format!("{}\n", line.text))
		.collect::<String>();
	let parts = parts(&text, MAX_PART_BYTES);
	let Some(selected) = parts.get(part - 1) else {
		return Ok(failure(operation, Failure::NoSuchPart));
	};
	let data = json!({
		"url": page.url,
		"title": page.title,
		"media_type": page.media_type,
		"retrieved_at": page.fetched_at,
		"completeness": page.completeness.as_str(),
		"part": part,
		"parts": parts.len(),
		"page_count": page.page_count,
		"pages_without_text": page.pages_without_text,
		"text": selected,
	});
	let result = envelope(operation, "ok", data, false);
	// A part and its metadata stay within the recorded result bound. Parts are
	// sized by their escaped length, and the part bound leaves room for the
	// address and page numbers, so this only guards a long title.
	if serde_json::to_vec(&result)?.len() > MAX_RESULT_BYTES {
		return Ok(failure(operation, Failure::ResponseTooLarge));
	}
	Ok(result)
}

#[cfg(test)]
mod tests;
