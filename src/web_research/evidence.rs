use super::{accounting, contracts::*, persistence};
use crate::{
	Error, Result,
	authorization::access::Access,
	capabilities::records::{self, Record},
	domain::Run,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use unicode_casefold::UnicodeCaseFold;
use uuid::Uuid;

pub(crate) async fn sources(access: &mut Access, run: &Run, result: &mut Value) -> Result<()> {
	let mut state = persistence::state(access, run).await?;
	if state["source_count"].as_u64().unwrap_or(0)
		+ result["data"]["sources"].as_array().map_or(0, Vec::len) as u64
		> 100
	{
		return Err(Error::Invalid("source_limit".into()));
	}
	if let Some(sources) = result["data"]["sources"].as_array_mut() {
		for source in sources {
			let id = Uuid::new_v4();
			let mut data = source.clone();
			data["run_id"] = json!(run.id);
			records::insert(access, id, None, "web.source", "unread", data, None).await?;
			source["source_id"] = json!(id);
			state["source_count"] = json!(state["source_count"].as_u64().unwrap_or(0) + 1);
		}
	}
	persistence::save_state(access, run, &state).await?;
	accounting::observation(access, run, result.to_string().len()).await
}
pub(crate) async fn document(
	access: &mut Access,
	run: &Run,
	input: &Value,
	extracted: &Value,
) -> Result<Value> {
	let extraction = &extracted["extraction"];
	if !matches!(
		extraction["state"].as_str(),
		Some("complete" | "text_limit")
	) {
		return Ok(failure(
			"web_open",
			"error",
			extraction["state"].as_str().unwrap_or("extraction_failed"),
		));
	}
	let open: Open = serde_json::from_value(input.clone())?;
	let mut state = persistence::state(access, run).await?;
	let size = extraction.to_string().len() as u64;
	if state["cache_bytes"]
		.as_u64()
		.unwrap_or(0)
		.saturating_add(size)
		> MAX_CACHE
	{
		return Ok(failure("web_open", "error", "cache_limit"));
	}
	let source = if let Some(source) = open.source_id {
		persistence::owned(access, run, source, "web.source").await?
	} else {
		records::insert(
			access,
			Uuid::new_v4(),
			None,
			"web.source",
			"unread",
			json!({"run_id":run.id,"url":extracted["url"],"title":extraction["title"]}),
			None,
		)
		.await?
	};
	let title = if extraction["title"].as_str().is_some_and(|t| !t.is_empty()) {
		extraction["title"].clone()
	} else {
		source.data["title"].clone()
	};
	let id = Uuid::new_v4();
	let record=records::insert(access,id,None,"web.document","ready",json!({"run_id":run.id,"source_id":source.id,
		"url":extracted["url"],"requested_url":open.url.as_ref().map_or_else(||source.data["url"].clone(),|url|json!(url)),"title":title,"fetched_at":extracted["fetched_at"],"raw_digest":extracted["raw_digest"],
		"text_digest":crate::registry::digest(extraction),"extraction_version":"aidash-web-extraction/1","completeness":extraction["state"],
		"media_type":extracted["media_type"],"lines":extraction["lines"],"size":size,"last_access":Utc::now()}),Some(Utc::now()+Duration::hours(24))).await?;
	state["cache_bytes"] = json!(state["cache_bytes"].as_u64().unwrap_or(0) + size);
	persistence::save_state(access, run, &state).await?;
	deliver(access, run, &record, 0, 0, open.validate()?, "web_open").await
}
async fn cached_document(access: &mut Access, run: &Run, id: Uuid) -> Result<Record> {
	let mut record = persistence::owned(access, run, id, "web.document").await?;
	persistence::owned(
		access,
		run,
		serde_json::from_value(record.data["source_id"].clone())?,
		"web.source",
	)
	.await?;
	if persistence::expired(&record) || record.data["lines"].is_null() {
		return Err(Error::Invalid("document_expired".into()));
	}
	record.expires_at = Some(Utc::now() + Duration::hours(24));
	record.data["last_access"] = json!(Utc::now());
	records::update(access, &mut record).await?;
	Ok(record)
}
pub(crate) async fn open(access: &mut Access, run: &Run, input: Open) -> Result<Value> {
	let limit = input.validate()?;
	let id = input.document_id.ok_or(Error::Forbidden)?;
	let record = cached_document(access, run, id).await?;
	let (line, offset) = if let Some(cursor) = input.cursor {
		let cursor = persistence::owned(access, run, cursor, "web.cursor").await?;
		if cursor.data["document_id"] != json!(id)
			|| cursor.data["operation"] != "web_open"
			|| persistence::expired(&cursor)
		{
			return Err(Error::Invalid("invalid_cursor".into()));
		}
		(
			cursor.data["line"].as_u64().ok_or(Error::Forbidden)? as usize,
			cursor.data["offset"].as_u64().ok_or(Error::Forbidden)? as usize,
		)
	} else {
		(0, 0)
	};
	deliver(access, run, &record, line, offset, limit, "web_open").await
}
fn folded(text: &str, sensitive: bool) -> String {
	if sensitive {
		text.to_owned()
	} else {
		text.case_fold().collect()
	}
}
pub(crate) async fn find(access: &mut Access, run: &Run, input: Find) -> Result<Value> {
	let maximum = input.validate()?;
	let document = cached_document(access, run, input.document_id).await?;
	let binding = crate::registry::digest(&json!([
		input.document_id,
		input.query,
		input.case_sensitive
	]));
	let start = if let Some(cursor) = input.cursor {
		let cursor = persistence::owned(access, run, cursor, "web.cursor").await?;
		if cursor.data["document_id"] != json!(document.id)
			|| cursor.data["operation"] != "web_find"
			|| cursor.data["binding"] != binding
			|| persistence::expired(&cursor)
		{
			return Err(Error::Invalid("invalid_cursor".into()));
		}
		cursor.data["line"].as_u64().ok_or(Error::Forbidden)? as usize
	} else {
		0
	};
	let needle = folded(&input.query, input.case_sensitive);
	let lines = document.data["lines"].as_array().ok_or(Error::Forbidden)?;
	let mut matches = vec![];
	let mut next = None;
	for (index, line) in lines.iter().enumerate().skip(start) {
		if !folded(
			line["text"].as_str().ok_or(Error::Forbidden)?,
			input.case_sensitive,
		)
		.contains(&needle)
		{
			continue;
		}
		if matches.len() >= maximum {
			next = Some(index);
			break;
		}
		let text = line["text"].as_str().ok_or(Error::Forbidden)?;
		// Keep both the combined JSON and every delivered excerpt bounded.
		let pages = if line["page"].is_null() {
			vec![]
		} else {
			vec![line["page"].clone()]
		};
		let candidate = fragment_data(
			run,
			&document,
			text,
			index + 1,
			index + 1,
			pages.clone(),
			Uuid::nil(),
		);
		if json!(matches).to_string().len() + candidate.to_string().len() + 1024 > MAX_ENVELOPE {
			if matches.is_empty() {
				return Err(Error::Invalid("find_excerpt_limit".into()));
			}
			next = Some(index);
			break;
		}
		matches.push(fragment(access, run, &document, text, index + 1, index + 1, pages).await?);
	}
	let cursor = if let Some(line) = next {
		Some(cursor(access, run, &document, "web_find", line, 0, Some(binding)).await?)
	} else {
		None
	};
	let mut result = envelope(
		"web_find",
		if matches.is_empty() {
			"empty"
		} else if next.is_some() {
			"partial"
		} else {
			"ok"
		},
		json!({"document_id":document.id,"matches":matches,"next_cursor":cursor,
		"case_folding_version":format!("Unicode {}.{}.{} full non-Turkic",unicode_casefold::UNICODE_VERSION.0,unicode_casefold::UNICODE_VERSION.1,unicode_casefold::UNICODE_VERSION.2)}),
	);
	result["limits"]["continuation"] = json!(cursor);
	result["limits"]["truncated"] = json!(next.is_some());
	Ok(result)
}
async fn cursor(
	access: &mut Access,
	run: &Run,
	document: &Record,
	operation: &str,
	line: usize,
	offset: usize,
	binding: Option<String>,
) -> Result<Uuid> {
	let id = Uuid::new_v4();
	records::insert(access,id,None,"web.cursor","ready",json!({"run_id":run.id,"document_id":document.id,"operation":operation,"line":line,"offset":offset,"binding":binding}),document.expires_at).await?;
	Ok(id)
}
async fn deliver(
	access: &mut Access,
	run: &Run,
	document: &Record,
	mut line: usize,
	mut offset: usize,
	limit: usize,
	operation: &str,
) -> Result<Value> {
	let lines = document.data["lines"].as_array().ok_or(Error::Forbidden)?;
	let first = line + 1;
	let mut last = first;
	let mut pages = vec![];
	let mut text = String::new();
	let mut all_pages = vec![];
	for line in lines {
		let page = &line["page"];
		if !page.is_null() && !all_pages.contains(page) {
			all_pages.push(page.clone());
		}
	}
	let mut probe = envelope(
		operation,
		"partial",
		fragment_data(run, document, "", first, 32768, all_pages, Uuid::nil()),
	);
	probe["data"]["next_cursor"] = json!(Uuid::nil());
	probe["limits"]["continuation"] = json!(Uuid::nil());
	probe["limits"]["truncated"] = json!(true);
	let mut json_budget = MAX_ENVELOPE.saturating_sub(probe.to_string().len() + 32);
	while line < lines.len() && text.len() < limit {
		let value = lines[line]["text"].as_str().ok_or(Error::Forbidden)?;
		if offset > value.len() || !value.is_char_boundary(offset) {
			return Err(Error::Invalid("invalid_cursor".into()));
		}
		let mut count = 0;
		for character in value[offset..].chars() {
			let bytes = character.len_utf8();
			let escaped = serde_json::to_string(&character)?.len() - 2;
			if count + bytes > limit - text.len() || escaped > json_budget {
				break;
			}
			count += bytes;
			json_budget -= escaped;
		}
		if count == 0 && offset < value.len() {
			break;
		}
		text.push_str(&value[offset..offset + count]);
		last = line + 1;
		if !lines[line]["page"].is_null() && !pages.contains(&lines[line]["page"]) {
			pages.push(lines[line]["page"].clone());
		}
		offset += count;
		if offset == value.len() {
			line += 1;
			offset = 0;
			if line < lines.len() && text.len() < limit {
				if json_budget < 2 {
					break;
				}
				text.push('\n');
				json_budget -= 2;
			}
		} else {
			break;
		}
	}
	if text.is_empty() && line < lines.len() {
		return Err(Error::Invalid("max_bytes_cannot_hold_character".into()));
	}
	let next = if line < lines.len() {
		Some(cursor(access, run, document, operation, line, offset, None).await?)
	} else {
		None
	};
	let fragment = fragment(access, run, document, &text, first, last, pages).await?;
	let partial = next.is_some() || document.data["completeness"] != "complete";
	let mut result = envelope(operation, if partial { "partial" } else { "ok" }, fragment);
	result["data"]["next_cursor"] = json!(next);
	result["limits"]["continuation"] = json!(next);
	result["limits"]["truncated"] = json!(partial);
	// JSON escaping can exceed a text-byte ceiling. Shrinking is explicit and
	// happens before exposing or committing any observation to the model.
	if result.to_string().len() > MAX_ENVELOPE {
		return Err(Error::Invalid("response_too_large".into()));
	}
	Ok(result)
}
async fn fragment(
	access: &mut Access,
	run: &Run,
	document: &Record,
	text: &str,
	start: usize,
	end: usize,
	pages: Vec<Value>,
) -> Result<Value> {
	let id = Uuid::new_v4();
	let data = fragment_data(run, document, text, start, end, pages, id);
	accounting::observation(access, run, data.to_string().len()).await?;
	records::insert(
		access,
		id,
		None,
		"web.evidence",
		"delivered",
		data.clone(),
		None,
	)
	.await?;
	Ok(data)
}
fn fragment_data(
	run: &Run,
	document: &Record,
	text: &str,
	start: usize,
	end: usize,
	pages: Vec<Value>,
	id: Uuid,
) -> Value {
	json!({"run_id":run.id,"source_id":document.data["source_id"],"document_id":document.id,
		"evidence_ref":format!("ev_{id}"),"url":document.data["url"],"requested_url":document.data["requested_url"],"title":document.data["title"],"fetched_at":document.data["fetched_at"],
		"raw_digest":document.data["raw_digest"],"text_digest":document.data["text_digest"],"extraction_version":document.data["extraction_version"],
		"completeness":document.data["completeness"],"text":text,"line_start":start,"line_end":end,"pdf_pages":pages})
}
pub(crate) async fn get(access: &mut Access, run: &Run, reference: &str) -> Result<Record> {
	let id = reference
		.strip_prefix("ev_")
		.and_then(|s| Uuid::parse_str(s).ok())
		.ok_or_else(|| Error::Invalid("invalid_evidence_reference".into()))?;
	let record = persistence::owned(access, run, id, "web.evidence").await?;
	persistence::owned(
		access,
		run,
		serde_json::from_value(record.data["source_id"].clone())?,
		"web.source",
	)
	.await?;
	if record.state != "delivered" || record.data["evidence_ref"] != reference {
		return Err(Error::NotFound("evidence unavailable".into()));
	}
	Ok(record)
}
pub(crate) fn references(text: &str) -> Result<Vec<String>> {
	let mut result = vec![];
	let mut remaining = text;
	while let Some((_, after)) = remaining.split_once("[[web:") {
		let (reference, rest) = after
			.split_once("]]")
			.ok_or_else(|| Error::Invalid("invalid_citation".into()))?;
		if reference.len() != 39
			|| !reference.starts_with("ev_")
			|| Uuid::parse_str(&reference[3..]).is_err()
		{
			return Err(Error::Invalid("invalid_citation".into()));
		}
		if !result.iter().any(|s| s == reference) {
			result.push(reference.to_owned());
		}
		if result.len() > 200 {
			return Err(Error::Invalid("citation_limit".into()));
		}
		remaining = rest;
	}
	Ok(result)
}
pub(crate) async fn validate_text(access: &mut Access, run: &Run, text: &str) -> Result<()> {
	for reference in references(text)? {
		get(access, run, &reference).await?;
	}
	Ok(())
}
pub(crate) async fn validate_result(access: &mut Access, run: &Run, value: &Value) -> Result<()> {
	fn collect(value: &Value, references: &mut Vec<String>) {
		match value {
			Value::Object(object) => {
				if let Some(reference) = object.get("evidence_ref").and_then(Value::as_str) {
					references.push(reference.to_owned());
				}
				for value in object.values() {
					collect(value, references);
				}
			}
			Value::Array(values) => {
				for value in values {
					collect(value, references);
				}
			}
			_ => {}
		}
	}
	let mut references = vec![];
	collect(value, &mut references);
	for reference in references {
		get(access, run, &reference).await?;
	}
	Ok(())
}
