//! Local embedding receipts retain unknown usage and bound refunds by reserved input.
use crate::{Error, Result, registry::EntityRef};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
	Query(Option<Uuid>),
	Index(Uuid),
}

/// Durable attempt metadata is independent of its ORM representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
	pub id: Uuid,
	pub workspace: Uuid,
	pub origin: Origin,
	pub provider: EntityRef,
	pub request_bytes: i64,
	pub amount: i64,
}

pub fn reservation_amount(request_bytes: usize) -> Result<i64> {
	request_bytes
		.checked_add(1024)
		.and_then(|amount| i64::try_from(amount).ok())
		.ok_or_else(|| Error::Invalid("embedding input reservation overflow".into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accounting {
	pub reported: Option<i64>,
	pub refund: i64,
	pub exceeded: bool,
}

/// The amount comes from a positive, validated reservation receipt.
pub fn accounting(amount: i64, reported: Option<u64>) -> Accounting {
	let valid = reported.filter(|tokens| *tokens > 0 && *tokens <= amount as u64);
	Accounting {
		reported: reported.and_then(|tokens| i64::try_from(tokens).ok()),
		refund: valid.map_or(0, |tokens| amount - tokens as i64),
		exceeded: reported.is_some_and(|tokens| tokens > amount as u64),
	}
}

#[cfg(test)]
mod tests;
