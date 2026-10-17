//! URL intents: a person's request, made with their input, that the Run read
//! particular pages. The host records the pages beside the person's text and
//! never in it (ADR 0052); a URL intent never approves the Web disclosure the
//! Page reading leads to.
use super::{Operation, search::candidate_host};
use crate::{
	registry::bindings::BindingSnapshot,
	tool::providers::{ToolDescriptor, research_operation},
};
use serde::{Deserialize, Serialize};

/// Most pages one input may ask the Run to read.
pub const MAX_URL_INTENTS: usize = 5;

/// Most pages carried forward from inputs the Run already took in. A Run
/// steered many times keeps only the newest of them, so the pages every
/// later step renders stay bounded however long it runs.
pub const MAX_CARRIED_PAGES: usize = 4 * MAX_URL_INTENTS;

/// The pages one step asks the model to read, each once and in order: the
/// Task's own; the newest [`MAX_CARRIED_PAGES`] of those asked for by the
/// inputs the Run already took in (`carried`, oldest first), which no page of
/// the step shows; and those of the inputs on this step (`current`).
pub fn requested_pages(
	task: &[String],
	carried: &[&[String]],
	current: &[&[String]],
) -> Vec<String> {
	fn add(pages: &mut Vec<String>, page: &str) {
		if !pages.iter().any(|known| known == page) {
			pages.push(page.to_owned());
		}
	}
	let mut pages = Vec::new();
	for page in task {
		add(&mut pages, page);
	}
	let mut newest: Vec<String> = Vec::new();
	for page in carried.iter().rev().flat_map(|pages| pages.iter().rev()) {
		if newest.len() == MAX_CARRIED_PAGES {
			break;
		}
		if !pages.contains(page) && !newest.contains(page) {
			newest.push(page.clone());
		}
	}
	pages.extend(newest.into_iter().rev());
	for page in current.iter().flat_map(|pages| pages.iter()) {
		add(&mut pages, page);
	}
	pages
}

/// What a request's step state tells the model about recorded URL intents.
/// It is rendered into every step that carries one and is never stored.
pub const URL_INTENT_INSTRUCTION: &str = "The person asked you to read these pages with the page reading tool before you answer. Each page still needs its own Web disclosure approval.";

/// The pages one input asked the Run to read: the Run's Task when
/// `input_key` is `None`, otherwise the run input with that idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlIntent {
	pub input_key: Option<String>,
	pub pages: Vec<String>,
}

/// Why a URL intent is refused. Nothing is recorded when any page is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidUrlIntent {
	TooMany,
	/// Not an absolute http(s) URL without credentials, fragment or control
	/// characters.
	Address(String),
	Duplicate(String),
	/// The Run's Agent binds no Page reading operation.
	NoPageReading,
}

impl std::fmt::Display for InvalidUrlIntent {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::TooMany => write!(f, "at most {MAX_URL_INTENTS} pages may be requested"),
			Self::Address(_) => write!(
				f,
				"a requested page must be an absolute http(s) URL without credentials or fragment"
			),
			Self::Duplicate(_) => write!(f, "a page is requested more than once"),
			Self::NoPageReading => write!(f, "the Agent binds no page reading operation"),
		}
	}
}

/// Checks the pages one input asks the Run to read, in the given order.
pub fn validate(pages: &[String]) -> Result<(), InvalidUrlIntent> {
	if pages.len() > MAX_URL_INTENTS {
		return Err(InvalidUrlIntent::TooMany);
	}
	for (index, page) in pages.iter().enumerate() {
		if candidate_host(page).is_none() {
			return Err(InvalidUrlIntent::Address(page.clone()));
		}
		if pages[..index].contains(page) {
			return Err(InvalidUrlIntent::Duplicate(page.clone()));
		}
	}
	Ok(())
}

/// Whether a Run with `snapshot` can act on a URL intent: its Agent binds a
/// Page reading operation, directly or through a bundle. Only the resolved
/// Bindings the Run is exposed to count, not every definition a bundle
/// declares: a member the bundle Binding did not select gives no tool. A
/// remote Run never can, because Research operations are never exposed to
/// remote Runs.
pub fn reads_pages(snapshot: &BindingSnapshot) -> bool {
	!snapshot.remote
		&& snapshot.bindings.iter().any(|resolved| {
			resolved.excluded_reason.is_none()
				&& resolved.definition.kind == "tool"
				&& serde_json::from_value::<ToolDescriptor>(resolved.definition.config.clone())
					.ok()
					.and_then(|tool| research_operation(&tool.provider, &tool.operation))
					.is_some_and(|(operation, _)| operation == Operation::Fetch)
		})
}

#[cfg(test)]
mod tests;
