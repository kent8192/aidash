//! One declared candidate keeps its identity, data trust and placement through
//! projection and complete-request fitting. Native defaults retain their layout.
use super::*;
use crate::{
	context::Context,
	projection::native::{Envelope, Source, Trust},
	provider::{ModelContext, ModelRequest},
};

pub(crate) struct Projection {
	pub snapshot: Value,
	pub run: Vec<Envelope>,
	pub step: Vec<Envelope>,
}

fn declared_envelope(contribution: &SourceContribution) -> Envelope {
	Envelope {
		source: Source::ContextSource,
		provenance: json!({
			"identity": contribution.identity,
			"sources": contribution.provenance,
			"dependencies": contribution.dependencies,
			"completeness": contribution.completeness,
			"detail": contribution.detail,
			"placement": contribution.placement,
			"fetched_digest": contribution.fetched_digest,
			"content_digest": contribution.content_digest,
		}),
		revision: None,
		trust: Trust::Data,
		content: contribution.content.clone(),
	}
}

fn key(slot: &str) -> &str {
	if slot == "references" {
		"reference_documents"
	} else {
		slot
	}
}

pub(crate) fn project(context: &Context, pinned: &Value) -> Projection {
	let mut projection = Projection {
		snapshot: pinned.clone(),
		run: vec![],
		step: vec![],
	};
	let mut removals = Vec::new();
	for contribution in &context.source_contributions {
		let Some(placement) = &contribution.placement else {
			continue;
		};
		let Some(volatility) = placement.volatility else {
			continue;
		};
		// Explicit Skill data was removed before the legacy instruction blocks
		// were composed. Other sources must still match the bounded snapshot;
		// a stale/clipped candidate must never be reintroduced from its record.
		if !placement.slot.ends_with("skills")
			&& pinned
				.get(key(&placement.slot))
				.and_then(|slot| slot.pointer(&placement.pointer))
				!= Some(&contribution.content)
		{
			continue;
		}
		let shared_default = context.source_contributions.iter().any(|other| {
			other.placement.as_ref().is_some_and(|other| {
				other.volatility.is_none()
					&& other.slot == placement.slot
					&& other.pointer == placement.pointer
			})
		});
		if !shared_default && !placement.slot.ends_with("skills") {
			removals.push((key(&placement.slot).to_owned(), placement.pointer.clone()));
		}
		match volatility {
			Volatility::Run => projection.run.push(declared_envelope(contribution)),
			Volatility::Step => projection.step.push(declared_envelope(contribution)),
		}
	}
	removals.sort_by(|left, right| {
		left.0
			.cmp(&right.0)
			.then_with(|| pointer_order(&right.1).cmp(&pointer_order(&left.1)))
	});
	removals.dedup();
	for (slot, pointer) in removals {
		if pointer.is_empty() {
			if let Some(fields) = projection.snapshot.as_object_mut() {
				fields.remove(&slot);
			}
		} else if let Some(value) = projection.snapshot.get_mut(&slot) {
			remove(value, &pointer);
		}
	}
	projection
}

fn pointer_order(pointer: &str) -> Vec<(String, usize)> {
	pointer
		.split('/')
		.skip(1)
		.map(|part| {
			(
				part.parse::<usize>()
					.map_or_else(|_| part.to_owned(), |_| String::new()),
				part.parse().unwrap_or(0),
			)
		})
		.collect()
}

fn remove(value: &mut Value, pointer: &str) {
	let Some((parent, field)) = pointer.rsplit_once('/') else {
		return;
	};
	match value.pointer_mut(parent) {
		Some(Value::Array(items)) => {
			if let Ok(index) = field.parse::<usize>()
				&& index < items.len()
			{
				// Keep the origin positions of other Sources. Their exact delivery
				// pointers must not shift when this candidate moves to an envelope.
				items[index] = Value::Null;
			}
		}
		Some(Value::Object(fields)) => {
			fields.remove(&field.replace("~1", "/").replace("~0", "~"));
		}
		_ => {}
	}
}

pub fn declared_present(request: &ModelRequest, contribution: &SourceContribution) -> bool {
	let Some(volatility) = contribution.placement.as_ref().and_then(|p| p.volatility) else {
		return false;
	};
	let envelope = declared_envelope(contribution);
	let contains = |part: &str| {
		serde_json::from_str::<Value>(part)
			.ok()
			.is_some_and(|part| contains_envelope(&part, &envelope))
	};
	match &request.context {
		ModelContext::Legacy(context) => context
			.get("current")
			.is_some_and(|part| contains_envelope(part, &envelope)),
		ModelContext::Ordered(context) => contains(match volatility {
			Volatility::Run => &context.stable,
			Volatility::Step => &context.volatile,
		}),
		ModelContext::Native(context) => match volatility {
			Volatility::Run => context.attachments.contains(&envelope),
			Volatility::Step => contains(&context.volatile),
		},
	}
}

fn contains_envelope(part: &Value, envelope: &Envelope) -> bool {
	let expected = serde_json::to_value(envelope).expect("serializable declared source");
	part.get("context_sources")
		.and_then(Value::as_array)
		.is_some_and(|items| items.contains(&expected))
}

/// Fit whole declared envelopes against the exact provider-neutral request.
/// Historical implicit content is untouched, and mandatory receipts never drop.
pub(crate) fn fit(request: &mut ModelRequest, contributions: &[SourceContribution], window: usize) {
	let candidates = contributions
		.iter()
		.filter(|item| declared_present(request, item))
		.collect::<Vec<_>>();
	if candidates.is_empty() || request.estimated_total_tokens() <= window {
		return;
	}
	let original = request.clone();
	let mut selected = candidates
		.iter()
		.filter(|item| item.mandatory)
		.map(|item| item.identity.key())
		.collect::<BTreeSet<_>>();
	retain(request, &selected);
	for item in candidates.into_iter().filter(|item| !item.mandatory) {
		let mut proposal = selected.clone();
		proposal.insert(item.identity.key());
		let mut candidate = original.clone();
		retain(&mut candidate, &proposal);
		if candidate.estimated_total_tokens() <= window {
			selected = proposal;
			*request = candidate;
		}
	}
}

fn retain(request: &mut ModelRequest, selected: &BTreeSet<String>) {
	let retained =
		|envelope: &Value| selected.contains(&digest(&envelope["provenance"]["identity"]));
	let filter = |value: &mut Value| {
		if let Some(items) = value
			.get_mut("context_sources")
			.and_then(Value::as_array_mut)
		{
			items.retain(&retained);
		}
	};
	let filter_text = |text: &mut String| {
		let mut value: Value = serde_json::from_str(text).expect("serialized projection part");
		filter(&mut value);
		*text = serde_json::to_string(&value).expect("serializable fitted projection part");
	};
	match &mut request.context {
		ModelContext::Legacy(context) => {
			if let Some(current) = context.get_mut("current") {
				filter(current);
			}
		}
		ModelContext::Ordered(context) => {
			filter_text(&mut context.stable);
			filter_text(&mut context.volatile);
		}
		ModelContext::Native(context) => {
			context.attachments.retain(|envelope| {
				envelope.source != Source::ContextSource
					|| selected.contains(&digest(&envelope.provenance["identity"]))
			});
			filter_text(&mut context.volatile);
		}
	}
}
