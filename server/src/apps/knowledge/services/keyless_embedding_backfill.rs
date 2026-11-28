//! A semantic index's Keyless embedding names `openai_compatible` (#1169), as
//! Reinhardt Query output materialized by `migrationseeds` for knowledge
//! 0060. The embedding of an index spec is the same configuration as an
//! embedding definition, so it follows registry 0083: an embedding without a
//! Provider Credential that said `openrouter` or `openai` now says
//! `openai_compatible`.
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

fn statement(from: &[&str], to: &str) -> String {
	let providers = from
		.iter()
		.map(|provider| format!("'{provider}'"))
		.collect::<Vec<_>>()
		.join(", ");
	let update = Query::update()
		.table(Alias::new("semantic_indexes"))
		.value_expr(
			Alias::new("spec"),
			Expr::cust(format!(
				"jsonb_set(\"spec\", '{{embedding,provider}}', '\"{to}\"')"
			)),
		)
		.and_where(Expr::cust(format!(
			"\"spec\" #>> '{{embedding,provider}}' IN ({providers})"
		)))
		.and_where(Expr::cust(
			"COALESCE(\"spec\" #> '{embedding,provider_credential}', 'null'::jsonb) = 'null'::jsonb",
		))
		.to_string(PostgresQueryBuilder);
	format!("{DEPLOYMENT_SCOPE}{update};\n")
}

/// Rewrites every Keyless index embedding to `openai_compatible`.
pub(crate) fn forward() -> String {
	statement(&["openrouter", "openai"], "openai_compatible")
}

/// Restores `openai`, the Keyless embedding provider every earlier version
/// accepted; which ones said `openrouter` is not recorded.
pub(crate) fn backward() -> String {
	statement(&["openai_compatible"], "openai")
}

#[cfg(test)]
mod tests {
	#[test]
	fn only_grant_less_embeddings_change() {
		assert!(super::forward().contains("IN ('openrouter', 'openai')"));
		assert!(super::forward().contains("provider_credential"));
		assert!(super::backward().contains("'\"openai\"'"));
	}
}
