//! URL intents: a person's request, made with their input, that the Run read
//! particular pages. The host renders them into a fixed instruction; a URL
//! intent never approves the Web disclosure the Page reading leads to.
use super::search::candidate_host;

/// Most pages one input may ask the Run to read.
pub const MAX_URL_INTENTS: usize = 5;

/// Opening line of the rendered instruction. The dashboard recognizes a URL
/// intent by this exact line.
pub const URL_INTENT_HEADING: &str = "Read these pages with the page reading tool before you answer. Each page still needs its own Web disclosure approval:";

/// Why a URL intent is refused. Nothing is rendered when any page is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidUrlIntent {
	TooMany,
	/// Not an absolute http(s) URL without credentials, fragment or control
	/// characters.
	Address(String),
	Duplicate(String),
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
		}
	}
}

/// `text` followed by the instruction to read `pages`, in the given order.
/// Without pages, `text` is returned unchanged.
pub fn render(text: &str, pages: &[String]) -> Result<String, InvalidUrlIntent> {
	if pages.is_empty() {
		return Ok(text.to_owned());
	}
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
	let mut rendered = text.trim_end().to_owned();
	if !rendered.is_empty() {
		rendered.push_str("\n\n");
	}
	rendered.push_str(URL_INTENT_HEADING);
	for page in pages {
		rendered.push_str("\n- ");
		rendered.push_str(page);
	}
	Ok(rendered)
}

#[cfg(test)]
mod tests;
