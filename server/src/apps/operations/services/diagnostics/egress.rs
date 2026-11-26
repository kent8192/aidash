//! The Egress gateway's open-file limits, for operators (#1140). This is a
//! runtime fact of the process, not a SQL query, so it has its own profile.

pub(super) fn document() -> serde_json::Value {
	serde_json::json!({
		"egress_open_files": egress_open_files(aidash_integrations::egress::open_file_limits()),
	})
}

fn egress_open_files(limits: Option<(u64, u64)>) -> serde_json::Value {
	let recommended = aidash_integrations::egress::RECOMMENDED_OPEN_FILES;
	match limits {
		Some((soft, hard)) => serde_json::json!({
			"soft": soft,
			"hard": hard,
			"recommended": recommended,
			"warning": (hard < recommended).then_some("Raise the node's LimitNOFILE"),
		}),
		None => serde_json::Value::Null,
	}
}

#[cfg(test)]
#[test]
fn warns_only_when_the_hard_limit_is_below_the_recommendation() {
	assert_eq!(
		egress_open_files(Some((1024, 65535)))["warning"],
		"Raise the node's LimitNOFILE"
	);
	assert!(egress_open_files(Some((1024, 65536)))["warning"].is_null());
}
