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
				write!(f, "; pin a leaf tag, one no other endpoint tag sits under")
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
mod tests;
