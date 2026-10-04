//! Provisional ownership permits reclamation without granting content disclosure.
pub fn provisional_kind(record: &str, object: &str) -> bool {
	match record {
		"transfer_out" => object == "transfer_snapshot",
		"transfer_in" => object == "transfer_staging",
		"reference" => matches!(
			object,
			"reference_staging" | "reference_original" | "reference_extraction"
		),
		_ => false,
	}
}
#[cfg(test)]
mod tests;
