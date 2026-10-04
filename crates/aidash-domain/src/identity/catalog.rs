//! Portable catalog approval state, separate from its persisted representation.
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(rename = "Binding")]
pub struct Binding {
	pub tenant: String,
	pub entry_id: String,
	pub entry_version: String,
	pub enabled: bool,
	pub revision: i64,
}

pub fn validate_revision(expected_revision: i64) -> crate::Result<()> {
	if !(0..i64::MAX).contains(&expected_revision) {
		return Err(crate::Error::Invalid("invalid catalog revision".into()));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	#[rstest::rstest]
	#[case::initial(0, true)]
	#[case::last_mutable(i64::MAX - 1, true)]
	#[case::negative(-1, false)]
	#[case::overflow(i64::MAX, false)]
	fn revisions_allow_the_initial_and_last_nonoverflowing_updates(
		#[case] expected: i64,
		#[case] allowed: bool,
	) {
		assert_eq!(super::validate_revision(expected).is_ok(), allowed);
	}
}
