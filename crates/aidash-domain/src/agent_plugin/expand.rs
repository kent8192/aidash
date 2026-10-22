const PLUGIN_ROOT: &str = "${PLUGIN_ROOT}";
const PLUGIN_DATA: &str = "${PLUGIN_DATA}";

/// Replaces every exact `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` in one left-to-right pass.
///
/// Replacement text is never scanned again, and other placeholder-like text stays literal.
pub fn expand(value: &str, plugin_root: &str, plugin_data: &str) -> String {
	let mut expanded = String::with_capacity(value.len());
	let mut rest = value;
	while let Some(start) = rest.find("${") {
		expanded.push_str(&rest[..start]);
		let tail = &rest[start..];
		if let Some(after) = tail.strip_prefix(PLUGIN_ROOT) {
			expanded.push_str(plugin_root);
			rest = after;
		} else if let Some(after) = tail.strip_prefix(PLUGIN_DATA) {
			expanded.push_str(plugin_data);
			rest = after;
		} else {
			expanded.push_str("${");
			rest = &tail[2..];
		}
	}
	expanded.push_str(rest);
	expanded
}
