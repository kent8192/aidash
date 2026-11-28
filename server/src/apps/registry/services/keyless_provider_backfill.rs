//! Stored Keyless model and embedding definitions name `openai_compatible`
//! (#1169), as Reinhardt Query output materialized by `migrationseeds` for
//! registry 0083. A Keyless definition names neither `provider_credential`
//! nor `plan_connection`; before #1169 its model said `openrouter` and its
//! embedding `openrouter` or `openai`.
//!
//! A Keyless model also loses what only an OpenRouter route can use: its
//! `provider_route` and an explicit `cache_mode`, which registration now
//! refuses on `openai_compatible`. Registered versions are rewritten in
//! place, Marketplace copies included (as registry/0054 did), and so are
//! pending registration requests.
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";
// Installed definitions have an immutable-writer fence; only the provider
// (and the OpenRouter-only keys of a Keyless model) change here.
const FENCE_OFF: &str = "ALTER TABLE public.registry DISABLE TRIGGER marketplace_registry_fence;\n";
const FENCE_ON: &str = "ALTER TABLE public.registry ENABLE TRIGGER marketplace_registry_fence;\n";

/// One table holding definitions: its name, the JSON column and the path of
/// the entry inside it (`metadata` holds `config` beside the row's `kind`
/// column; a request holds the whole entry, `kind` included).
struct Holder {
	table: &'static str,
	column: &'static str,
	kind: &'static str,
}

const HOLDERS: [Holder; 2] = [
	Holder {
		table: "registry",
		column: "metadata",
		kind: "\"kind\"",
	},
	Holder {
		table: "registry_requests",
		column: "request",
		kind: "\"request\" ->> 'kind'",
	},
];

fn without_grant(column: &str) -> String {
	format!(
		"COALESCE(\"{column}\" #> '{{config,provider_credential}}', 'null'::jsonb) = 'null'::jsonb \
		 AND COALESCE(\"{column}\" #> '{{config,plan_connection}}', 'null'::jsonb) = 'null'::jsonb"
	)
}

fn rewrite(holder: &Holder, kind: &str, from: &[&str], config: String) -> String {
	let column = holder.column;
	let providers = from
		.iter()
		.map(|provider| format!("'{provider}'"))
		.collect::<Vec<_>>()
		.join(", ");
	Query::update()
		.table(Alias::new(holder.table))
		.value_expr(
			Alias::new(column),
			Expr::cust(format!("jsonb_set(\"{column}\", '{{config}}', {config})")),
		)
		.and_where(Expr::cust(format!("{} = '{kind}'", holder.kind)))
		.and_where(Expr::cust(format!(
			"\"{column}\" #>> '{{config,provider}}' IN ({providers})"
		)))
		.and_where(Expr::cust(without_grant(column)))
		.to_string(PostgresQueryBuilder)
}

fn statements(
	model_from: &[&str],
	model_config: impl Fn(&str) -> String,
	embedding_from: &[&str],
	embedding_to: &str,
) -> String {
	let mut sql = String::new();
	for holder in &HOLDERS {
		if holder.table == "registry" {
			sql.push_str(FENCE_OFF);
		}
		for statement in [
			rewrite(holder, "model", model_from, model_config(holder.column)),
			rewrite(
				holder,
				"embedding",
				embedding_from,
				format!(
					"jsonb_set(\"{}\" -> 'config', '{{provider}}', '\"{embedding_to}\"')",
					holder.column
				),
			),
		] {
			sql.push_str(&statement);
			sql.push_str(";\n");
		}
		if holder.table == "registry" {
			sql.push_str(FENCE_ON);
		}
	}
	format!("{DEPLOYMENT_SCOPE}{sql}")
}

/// Rewrites every Keyless model and embedding definition to `openai_compatible`.
pub(crate) fn forward() -> String {
	statements(
		&["openrouter"],
		|column| {
			format!(
				"((CASE WHEN \"{column}\" #>> '{{config,cache_mode}}' = 'explicit' \
				 THEN (\"{column}\" -> 'config') - 'cache_mode' ELSE \"{column}\" -> 'config' END) \
				 - 'provider_route') || '{{\"provider\":\"openai_compatible\"}}'::jsonb"
			)
		},
		&["openrouter", "openai"],
		"openai_compatible",
	)
}

/// Restores the provider each kind accepted before 0083: `openrouter` for a
/// model and `openai` for an embedding. Which Keyless embeddings said
/// `openrouter`, and the removed `provider_route` and explicit `cache_mode`,
/// are not recorded, so they are not restored.
pub(crate) fn backward() -> String {
	statements(
		&["openai_compatible"],
		|column| format!("jsonb_set(\"{column}\" -> 'config', '{{provider}}', '\"openrouter\"')"),
		&["openai_compatible"],
		"openai",
	)
}

#[cfg(test)]
mod tests {
	#[test]
	fn only_grant_less_definitions_change() {
		let forward = super::forward();
		assert_eq!(forward.matches("UPDATE").count(), 4);
		assert_eq!(forward.matches("provider_credential").count(), 4);
		assert!(forward.contains("- 'provider_route'"));
		assert!(forward.contains("IN ('openrouter', 'openai')"));
		assert!(super::backward().contains("'\"openrouter\"'"));
	}
}
