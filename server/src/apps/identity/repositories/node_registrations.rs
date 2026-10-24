//! Node Registrations of a Tenant's Nodes and their history (#241). Every
//! statement runs in the Tenant's scope; only `name_tenant` reads the
//! Deployment-wide `node_names`.
use crate::{Error, Result};
use aidash_domain::nodes::{
	NodeAddress, NodeName,
	registration::{Counterpart, NodeRegistration},
};
use aidash_domain::tenancy::TenantName;
use reinhardt::db::backends::{QueryValue, Row, TransactionExecutor};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

const COLUMNS: [&str; 12] = [
	"tenant",
	"node",
	"source_node",
	"source_endpoint",
	"source_public_key_pem",
	"source_key_id",
	"atomic_transactions",
	"enabled",
	"revision",
	"actor",
	"created_at",
	"updated_at",
];

/// A nullable text column; `Row::get` reads no `Option`.
fn optional_text(row: &Row, column: &str) -> Result<Option<String>> {
	match row.data.get(column) {
		None | Some(QueryValue::Null) => Ok(None),
		Some(value) => String::try_from(value.clone())
			.map(Some)
			.map_err(|error| reinhardt::core::exception::Error::from(error).into()),
	}
}

fn registration_of(row: &Row) -> Result<NodeRegistration> {
	let failed = reinhardt::core::exception::Error::from;
	let tenant: String = row.get("tenant").map_err(failed)?;
	let node: String = row.get("node").map_err(failed)?;
	let source_node: String = row.get("source_node").map_err(failed)?;
	Ok(NodeRegistration {
		tenant: TenantName::new(tenant)?,
		node: NodeName::parse(&node)?,
		source_node: NodeAddress::parse(&source_node)?,
		counterpart: Counterpart::from_columns(
			optional_text(row, "source_endpoint")?,
			optional_text(row, "source_public_key_pem")?,
			optional_text(row, "source_key_id")?,
		)?,
		atomic_transactions: row.get("atomic_transactions").map_err(failed)?,
		enabled: row.get("enabled").map_err(failed)?,
		revision: row.get("revision").map_err(failed)?,
		actor: row.get("actor").map_err(failed)?,
		created_at: row.get("created_at").map_err(failed)?,
		updated_at: row.get("updated_at").map_err(failed)?,
	})
}

/// What a write sets. The revision, the actor's stamp and the times are the
/// repository's.
pub(crate) struct Write<'a> {
	pub tenant: &'a str,
	pub node: &'a str,
	pub source_node: &'a NodeAddress,
	pub counterpart: &'a Counterpart,
	pub atomic_transactions: bool,
	pub enabled: bool,
	pub actor: &'a str,
	/// Permission edits/disabling retain the latest independently advanced pin.
	pub preserve_keys: bool,
}

/// The Tenant that holds `name`, from the Deployment-wide record of Node names.
pub(crate) async fn name_tenant(
	tx: &mut dyn TransactionExecutor,
	name: &str,
) -> Result<Option<String>> {
	let (sql, values) = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("node_names"))
		.and_where(Expr::col("name").eq(Expr::value(name)))
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.map(|row| {
			row.get("tenant")
				.map_err(|error| Error::from(reinhardt::core::exception::Error::from(error)))
		})
		.transpose()
}

pub(crate) async fn list(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	offset: u64,
	limit: u64,
) -> Result<Vec<NodeRegistration>> {
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("node_registrations"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("node").eq(Expr::value(node)))
		.order_by(Alias::new("source_node"), Order::Asc)
		.offset(offset)
		.limit(limit)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(registration_of)
		.collect()
}

pub(crate) async fn history(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	after: i64,
	limit: u64,
) -> Result<Vec<(i64, NodeRegistration)>> {
	let (sql, values) = Query::select()
		.column(Alias::new("sequence"))
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("node_registration_history"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("node").eq(Expr::value(node)))
		.and_where(Expr::col("sequence").gt(Expr::value(after)))
		.order_by(Alias::new("sequence"), Order::Asc)
		.limit(limit)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(|row| {
			let sequence: i64 = row
				.get("sequence")
				.map_err(reinhardt::core::exception::Error::from)?;
			Ok((sequence, registration_of(row)?))
		})
		.collect()
}

/// The registration at `node` for `source`, if there is one.
pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	source: &NodeAddress,
) -> Result<Option<NodeRegistration>> {
	get_locked(tx, tenant, node, source, false).await
}

/// [`get`], holding a shared lock until the transaction ends when `lock` is set,
/// so a read that depends on the registration finishes before it is changed.
pub(crate) async fn get_locked(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	source: &NodeAddress,
	lock: bool,
) -> Result<Option<NodeRegistration>> {
	let mut query = Query::select();
	query
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("node_registrations"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("node").eq(Expr::value(node)))
		.and_where(Expr::col("source_node").eq(Expr::value(source.to_string())));
	if lock {
		query.lock(reinhardt::query::LockType::Share);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.as_ref()
		.map(registration_of)
		.transpose()
}

/// Creates the registration at revision 1. `None` when one already exists.
pub(crate) async fn insert(
	tx: &mut dyn TransactionExecutor,
	write: &Write<'_>,
) -> Result<Option<NodeRegistration>> {
	let (endpoint, pem, key_id) = write.counterpart.columns();
	let source_keys = match write.counterpart {
		Counterpart::SameDeployment => None,
		Counterpart::OtherDeployment { key, .. } => Some(serde_json::to_string(
			&aidash_application::federation::registrations::trusted_keys(key)?,
		)?),
	};
	let (sql, values) = Query::insert()
		.into_table(Alias::new("node_registrations"))
		.columns(
			[
				"tenant",
				"node",
				"source_node",
				"source_endpoint",
				"source_public_key_pem",
				"source_key_id",
				"source_keys",
				"atomic_transactions",
				"enabled",
				"revision",
				"actor",
			]
			.map(Alias::new),
		)
		.values_panic([
			IntoValue::into_value(write.tenant.to_owned()),
			IntoValue::into_value(write.node.to_owned()),
			IntoValue::into_value(write.source_node.to_string()),
			IntoValue::into_value(endpoint.map(str::to_owned)),
			IntoValue::into_value(pem.map(str::to_owned)),
			IntoValue::into_value(key_id.map(str::to_owned)),
			IntoValue::into_value(source_keys),
			IntoValue::into_value(write.atomic_transactions),
			IntoValue::into_value(write.enabled),
			IntoValue::into_value(1_i64),
			IntoValue::into_value(write.actor.to_owned()),
		])
		.on_conflict(
			OnConflict::columns(["tenant", "node", "source_node"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.returning(COLUMNS.map(Alias::new))
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.as_ref()
		.map(registration_of)
		.transpose()
}

/// Replaces the registration when it is still at `expected_revision`. `None`
/// when it is not, so a stale writer answers 409.
pub(crate) async fn update(
	tx: &mut dyn TransactionExecutor,
	write: &Write<'_>,
	expected_revision: i64,
) -> Result<Option<NodeRegistration>> {
	let mut query = Query::update();
	query.table(Alias::new("node_registrations"));
	if !write.preserve_keys {
		let (endpoint, _, _) = write.counterpart.columns();
		query.value(
			Alias::new("source_endpoint"),
			Expr::value(endpoint.map(str::to_owned)),
		);
		if let Counterpart::OtherDeployment { key, .. } = write.counterpart {
			let owner =
				aidash_application::ports::federation::inbound::ReceivingKeyOwner::Registration {
					tenant: TenantName::new(write.tenant)?,
					node: NodeName::parse(write.node)?,
					source_node: write.source_node.clone(),
				};
			let stored = super::receiving_keys::get_locked(tx, &owner).await?;
			if stored.is_none_or(|stored| stored.keys.current.key_id != key.key_id) {
				return Err(Error::Conflict(
					"receiving key changed; use independent confirmation".into(),
				));
			}
		}
		// Existing trust is never reset by a permission/endpoint edit.
	}
	let (sql, values) = query
		.value(
			Alias::new("atomic_transactions"),
			Expr::value(write.atomic_transactions),
		)
		.value(Alias::new("enabled"), Expr::value(write.enabled))
		.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
		.value(Alias::new("actor"), Expr::value(write.actor.to_owned()))
		.value_expr(Alias::new("updated_at"), Expr::cust("clock_timestamp()"))
		.and_where(Expr::col("tenant").eq(Expr::value(write.tenant)))
		.and_where(Expr::col("node").eq(Expr::value(write.node)))
		.and_where(Expr::col("source_node").eq(Expr::value(write.source_node.to_string())))
		.and_where(Expr::col("revision").eq(Expr::value(expected_revision)))
		.returning(COLUMNS.map(Alias::new))
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.as_ref()
		.map(registration_of)
		.transpose()
}

pub(crate) async fn append_history(
	tx: &mut dyn TransactionExecutor,
	registration: &NodeRegistration,
) -> Result<()> {
	// Snapshot the latest crypto state with the authority revision's history.
	// Crypto-only updates themselves do not append authorization history.
	let (sql, values) = Query::insert()
		.into_table(Alias::new("node_registration_history"))
		.columns(
			COLUMNS
				.into_iter()
				.chain(["source_keys", "keys_revision"])
				.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.columns(
					COLUMNS
						.into_iter()
						.chain(["source_keys", "keys_revision"])
						.map(Alias::new),
				)
				.from(Alias::new("node_registrations"))
				.and_where(Expr::col("tenant").eq(Expr::value(registration.tenant.as_str())))
				.and_where(Expr::col("node").eq(Expr::value(registration.node.as_str())))
				.and_where(
					Expr::col("source_node").eq(Expr::value(registration.source_node.to_string())),
				)
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// The enabled registrations at `node`: the Nodes it addresses.
pub(crate) async fn enabled_of(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Vec<NodeRegistration>> {
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("node_registrations"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("node").eq(Expr::value(node)))
		.and_where(Expr::col("enabled").eq(Expr::value(true)))
		.order_by(Alias::new("source_node"), Order::Asc)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(registration_of)
		.collect()
}
