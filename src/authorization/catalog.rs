use super::{Authorization, access::Access, identity::SubjectIdentity};
use crate::{
    Error, Result,
    registry::{EntityRef, Entry, Search},
    store::Store,
};
use sea_orm::sea_query::{
    Alias, Asterisk, Condition, Expr, JoinType, LockType, OnConflict, Order, PostgresQueryBuilder,
    Query,
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Binding {
    pub tenant: String,
    pub entry_id: String,
    pub entry_version: String,
    pub enabled: bool,
    pub revision: i64,
}

impl Authorization {
    pub async fn set_catalog(
        &self,
        tenant: &str,
        entry: &EntityRef,
        expected_revision: i64,
        enabled: bool,
        actor: &str,
    ) -> Result<Binding> {
        if !(0..i64::MAX).contains(&expected_revision) {
            return Err(Error::Invalid("invalid catalog revision".into()));
        }
        let mut tx = self.pool.begin().await?;
        Self::load(&mut tx, tenant).await?;
        let exists: bool = sqlx::query_scalar(
            &Query::select()
                .expr(Expr::exists(
                    Query::select()
                        .expr(Expr::cust("1"))
                        .from(Alias::new("registry"))
                        .cond_where(
                            Condition::all()
                                .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                                .add(Expr::col(Alias::new("version")).eq(Expr::cust("$2"))),
                        )
                        .to_owned(),
                ))
                .to_string(PostgresQueryBuilder),
        )
        .bind(&entry.id)
        .bind(&entry.version)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(Error::NotFound("registry entry".into()));
        }
        let binding: Option<Binding> = if expected_revision == 0 {
            sqlx::query_as(
                &Query::insert()
                    .into_table(Alias::new("authorization_catalog"))
                    .columns([
                        Alias::new("tenant"),
                        Alias::new("entry_id"),
                        Alias::new("entry_version"),
                        Alias::new("enabled"),
                        Alias::new("revision"),
                    ])
                    .values_panic([
                        Expr::cust("$1"),
                        Expr::cust("$2"),
                        Expr::cust("$3"),
                        Expr::cust("$4"),
                        Expr::cust("1"),
                    ])
                    .on_conflict(OnConflict::new().do_nothing().to_owned())
                    .returning_all()
                    .to_string(PostgresQueryBuilder),
            )
            .bind(tenant)
            .bind(&entry.id)
            .bind(&entry.version)
            .bind(enabled)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            sqlx::query_as(
                &Query::update()
                    .table(Alias::new("authorization_catalog"))
                    .value(Alias::new("enabled"), Expr::cust("$4"))
                    .value(Alias::new("revision"), Expr::cust("revision+1"))
                    .cond_where(
                        Condition::all()
                            .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                            .add(Expr::col(Alias::new("entry_id")).eq(Expr::cust("$2")))
                            .add(Expr::col(Alias::new("entry_version")).eq(Expr::cust("$3")))
                            .add(Expr::col(Alias::new("revision")).eq(Expr::cust("$5"))),
                    )
                    .returning_all()
                    .to_string(PostgresQueryBuilder),
            )
            .bind(tenant)
            .bind(&entry.id)
            .bind(&entry.version)
            .bind(enabled)
            .bind(expected_revision)
            .fetch_optional(&mut *tx)
            .await?
        };
        let binding = binding.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
        sqlx::query(
            &Query::insert()
                .into_table(Alias::new("authorization_catalog_history"))
                .columns([
                    Alias::new("tenant"),
                    Alias::new("entry_id"),
                    Alias::new("entry_version"),
                    Alias::new("revision"),
                    Alias::new("enabled"),
                    Alias::new("actor"),
                ])
                .values_panic([
                    Expr::cust("$1"),
                    Expr::cust("$2"),
                    Expr::cust("$3"),
                    Expr::cust("$4"),
                    Expr::cust("$5"),
                    Expr::cust("$6"),
                ])
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .bind(&entry.id)
        .bind(&entry.version)
        .bind(binding.revision)
        .bind(enabled)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(binding)
    }

    pub async fn catalog(&self, tenant: &str) -> Result<Vec<Binding>> {
        Ok(sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("authorization_catalog"))
                .cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                .order_by(Alias::new("entry_id"), Order::Asc)
                .order_by(Alias::new("entry_version"), Order::Asc)
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .fetch_all(&self.pool)
        .await?)
    }
}

pub(crate) async fn entry(
    access: &mut Access,
    reference: &EntityRef,
    action: &str,
) -> Result<Entry> {
    let key = (reference.id.clone(), reference.version.clone());
    if access.inherited_lease && !access.approved_catalog.contains(&key) {
        return Err(Error::Forbidden);
    }
    let query = if access.inherited_lease {
        Query::select()
            .column((Alias::new("r"), Alias::new("metadata")))
            .from_as(Alias::new("authorization_catalog"), Alias::new("c"))
            .join_as(
                JoinType::InnerJoin,
                Alias::new("registry"),
                Alias::new("r"),
                Condition::all()
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("id")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
                    )
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("version")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
                    ),
            )
            .cond_where(
                Condition::all()
                    .add(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
                    .add(Expr::col((Alias::new("c"), Alias::new("entry_id"))).eq(Expr::cust("$2")))
                    .add(
                        Expr::col((Alias::new("c"), Alias::new("entry_version")))
                            .eq(Expr::cust("$3")),
                    )
                    .add(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true)),
            )
            .to_string(PostgresQueryBuilder)
    } else {
        Query::select()
            .column((Alias::new("r"), Alias::new("metadata")))
            .from_as(Alias::new("authorization_catalog"), Alias::new("c"))
            .join_as(
                JoinType::InnerJoin,
                Alias::new("registry"),
                Alias::new("r"),
                Condition::all()
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("id")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
                    )
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("version")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
                    ),
            )
            .cond_where(
                Condition::all()
                    .add(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
                    .add(Expr::col((Alias::new("c"), Alias::new("entry_id"))).eq(Expr::cust("$2")))
                    .add(
                        Expr::col((Alias::new("c"), Alias::new("entry_version")))
                            .eq(Expr::cust("$3")),
                    )
                    .add(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true)),
            )
            .lock_with_tables(LockType::Share, [Alias::new("c")])
            .to_string(PostgresQueryBuilder)
    };
    let document: Option<Value> = sqlx::query_scalar(&query)
        .bind(&access.identity.tenant)
        .bind(&reference.id)
        .bind(&reference.version)
        .fetch_optional(&mut *access.tx)
        .await?;
    let entry: Entry = serde_json::from_value(document.ok_or(Error::Forbidden)?)?;
    access.require(&resource(access, &entry), action).await?;
    access.approved_catalog.insert(key);
    Ok(entry)
}

pub(crate) fn resource(access: &Access, entry: &Entry) -> super::policy::Resource {
    access.resource(&entry.kind,&entry.id,json!({"version":entry.version,"capabilities":entry.capabilities,"tags":entry.tags,"languages":entry.languages,"config":entry.config}))
}

pub(crate) async fn list_in(access: &mut Access, search: &Search) -> Result<Vec<Entry>> {
    let query = if access.inherited_lease {
        Query::select()
            .column((Alias::new("r"), Alias::new("metadata")))
            .from_as(Alias::new("authorization_catalog"), Alias::new("c"))
            .join_as(
                JoinType::InnerJoin,
                Alias::new("registry"),
                Alias::new("r"),
                Condition::all()
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("id")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
                    )
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("version")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
                    ),
            )
            .cond_where(
                Condition::all()
                    .add(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
                    .add(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true)),
            )
            .order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
            .order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
            .to_string(PostgresQueryBuilder)
    } else {
        Query::select()
            .column((Alias::new("r"), Alias::new("metadata")))
            .from_as(Alias::new("authorization_catalog"), Alias::new("c"))
            .join_as(
                JoinType::InnerJoin,
                Alias::new("registry"),
                Alias::new("r"),
                Condition::all()
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("id")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
                    )
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("version")))
                            .eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
                    ),
            )
            .cond_where(
                Condition::all()
                    .add(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
                    .add(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true)),
            )
            .order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
            .order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
            .lock_with_tables(LockType::Share, [Alias::new("c")])
            .to_string(PostgresQueryBuilder)
    };
    let documents: Vec<Value> = sqlx::query_scalar(&query)
        .bind(&access.identity.tenant)
        .fetch_all(&mut *access.tx)
        .await?;
    let mut entries = vec![];
    for document in documents {
        let entry: Entry = serde_json::from_value(document)?;
        if (!access.inherited_lease
            || access
                .approved_catalog
                .contains(&(entry.id.clone(), entry.version.clone())))
            && search.matches(&entry)
            && access
                .decide(&resource(access, &entry), "registry.read")
                .await?
        {
            access
                .approved_catalog
                .insert((entry.id.clone(), entry.version.clone()));
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub async fn list(
    store: &Store,
    identity: &SubjectIdentity,
    search: &Search,
) -> Result<Vec<Entry>> {
    let mut access = Access::begin(store, identity).await?;
    let result = list_in(&mut access, search).await;
    access.finish(result).await
}

pub async fn get(
    store: &Store,
    identity: &SubjectIdentity,
    reference: &EntityRef,
) -> Result<Entry> {
    let mut access = Access::begin(store, identity).await?;
    let result = entry(&mut access, reference, "registry.read").await;
    access.finish(result).await
}
