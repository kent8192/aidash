//! Reasons an approved provider or media route cannot serve a request.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteReason {
	NotYetValid,
	Expired,
	MediaFormatsNotApproved { formats: Vec<String> },
	SubTags { tags: Vec<String> },
	EndpointMissing,
	ContextTooLong { estimated: u64, limit: u64 },
	NotZeroDataRetention,
}

impl fmt::Display for RouteReason {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::NotYetValid => write!(f, "approval evidence is not yet valid"),
			Self::Expired => write!(f, "approval evidence has expired"),
			Self::MediaFormatsNotApproved { formats } => {
				write!(f, "media formats not approved: {}", formats.join(", "))
			}
			Self::SubTags { tags } => {
				let mut sorted = tags.clone();
				sorted.sort();
				write!(
					f,
					"sub-tags exist: {}",
					sorted[..sorted.len().min(5)].join(", ")
				)?;
				if sorted.len() > 5 {
					write!(f, ", and {} more", sorted.len() - 5)?;
				}
				write!(
					f,
					"; pin a leaf tag (one with no sub-tags), such as google-vertex/global/priority"
				)
			}
			Self::EndpointMissing => write!(f, "endpoint missing from the catalog"),
			Self::ContextTooLong { estimated, limit } => write!(
				f,
				"context too long: estimated {estimated} tokens > limit {limit}"
			),
			Self::NotZeroDataRetention => write!(f, "not on the zero data retention list"),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRefusal {
	pub tag: String,
	pub reasons: Vec<RouteReason>,
}

impl fmt::Display for RouteRefusal {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}: ", self.tag)?;
		for (index, reason) in self.reasons.iter().enumerate() {
			if index != 0 {
				write!(f, "; ")?;
			}
			write!(f, "{reason}")?;
		}
		Ok(())
	}
}

pub struct CandidateRefusals<'a>(pub &'a [RouteRefusal]);
impl fmt::Display for CandidateRefusals<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		for (index, refusal) in self.0.iter().enumerate() {
			write!(f, "{}{}", if index == 0 { ": " } else { "; " }, refusal)?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sub_tags_are_sorted_and_truncated_for_display() {
		let refusal = RouteRefusal {
			tag: "google-vertex/global".into(),
			reasons: vec![RouteReason::SubTags {
				tags: ["z", "d", "b", "a", "c", "e", "f"]
					.into_iter()
					.map(|s| format!("google-vertex/global/{s}"))
					.collect(),
			}],
		};
		let message = refusal.to_string();
		assert!(message.contains("google-vertex/global/a, google-vertex/global/b, google-vertex/global/c, google-vertex/global/d, google-vertex/global/e, and 2 more"), "{message}");
		assert!(!message.contains("google-vertex/global/f"));
		assert!(message.contains("leaf tag"));
	}
}
