use crate::{
    Error, Result,
    authorization::policy::{PolicyBundle, identifier},
    registry::{AgentConfig, EntityRef, Entry, ModelConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationPermissions)]
pub struct Permissions {
    #[serde(default)]
    pub roles: BTreeSet<String>,
    #[serde(default)]
    pub groups: BTreeSet<String>,
    #[serde(default = "crate::domain::empty_object")]
    #[schema(value_type=std::collections::BTreeMap<String,Value>)]
    pub attributes: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationLimits)]
pub struct Limits {
    pub max_agents: i64,
    pub max_concurrent: i64,
    pub max_depth: i32,
    pub token_budget: i64,
    pub tokens_per_agent: i64,
    pub lifetime_seconds: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationCompaction)]
pub struct Compaction {
    pub provider: EntityRef,
    pub calls_per_agent: i64,
    pub call_budget: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationSpec)]
pub struct Spec {
    pub enabled: bool,
    pub template: Entry,
    pub permissions: Permissions,
    pub limits: Limits,
    pub approval_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<Compaction>,
}
#[derive(Serialize, utoipa::ToSchema)]
#[schema(as = GenerationPolicy)]
pub struct Policy {
    pub tenant: String,
    pub id: String,
    pub revision: i64,
    pub spec: Spec,
    pub generated_count: i64,
    pub allocated_tokens: i64,
    pub allocated_compaction_calls: i64,
}

impl Spec {
    pub fn validate(&self, bundle: &PolicyBundle) -> Result<AgentConfig> {
        crate::registry::validate(&self.template)?;
        if self.template.kind != "agent" {
            return Err(Error::Invalid(
                "generation template must define an agent".into(),
            ));
        }
        if self.compaction.as_ref().is_some_and(|c| {
            !(1..=1_000_000).contains(&c.call_budget)
                || !(1..=c.call_budget).contains(&c.calls_per_agent)
        }) {
            return Err(Error::Invalid("invalid compaction call limits".into()));
        }
        let limits = &self.limits;
        if !(1..=512).contains(&limits.max_agents)
            || !(1..=limits.max_agents).contains(&limits.max_concurrent)
            || !(1..=31).contains(&limits.max_depth)
            || !(1..=1_000_000_000_000_i64).contains(&limits.token_budget)
            || !(1..=limits.token_budget).contains(&limits.tokens_per_agent)
            || !(1..=2_592_000).contains(&limits.lifetime_seconds)
        {
            return Err(Error::Invalid("invalid generation limits".into()));
        }
        if !self.permissions.attributes.is_object()
            || self.permissions.attributes.to_string().len() > 16_384
        {
            return Err(Error::Invalid(
                "generated attributes must be an object of at most 16 KiB".into(),
            ));
        }
        for role in &self.permissions.roles {
            if !bundle.roles.contains_key(role) {
                return Err(Error::Invalid("generated role does not exist".into()));
            }
        }
        for group in &self.permissions.groups {
            if !bundle.groups.contains_key(group) {
                return Err(Error::Invalid("generated group does not exist".into()));
            }
        }
        Ok(serde_json::from_value(self.template.config.clone())?)
    }
}

pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &str,
    id: &str,
    exclusive: bool,
) -> Result<Policy> {
    let query = if exclusive {
        "SELECT revision,spec,generated_count,allocated_tokens,allocated_compaction_calls FROM generation_policies WHERE tenant=$1 AND id=$2 FOR UPDATE"
    } else {
        "SELECT revision,spec,generated_count,allocated_tokens,allocated_compaction_calls FROM generation_policies WHERE tenant=$1 AND id=$2 FOR SHARE"
    };
    let row: Option<(i64, Value, i64, i64, i64)> = sqlx::query_as(query)
        .bind(tenant)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let (revision, spec, generated_count, allocated_tokens, allocated_compaction_calls) =
        row.ok_or_else(|| Error::NotFound("generation policy".into()))?;
    Ok(Policy {
        tenant: tenant.into(),
        id: id.into(),
        revision,
        spec: serde_json::from_value(spec)?,
        generated_count,
        allocated_tokens,
        allocated_compaction_calls,
    })
}

pub(crate) async fn write(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &str,
    id: &str,
    expected: i64,
    spec: &Spec,
    actor: &str,
) -> Result<Policy> {
    identifier(tenant)?;
    identifier(id)?;
    identifier(actor)?;
    if !(0..i64::MAX).contains(&expected) {
        return Err(Error::Invalid("invalid generation policy revision".into()));
    }
    let document: Value =
        sqlx::query_scalar("SELECT document FROM authorization_bundles WHERE tenant=$1 FOR UPDATE")
            .bind(tenant)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| Error::NotFound("authorization policy".into()))?;
    // Revocation must never prevent an otherwise unchanged policy from being
    // disabled. Re-enabling or editing still validates every live dependency.
    let disabling = if expected > 0 && !spec.enabled {
        let previous: Option<Value> = sqlx::query_scalar(
            "SELECT spec FROM generation_policies WHERE tenant=$1 AND id=$2 AND revision=$3",
        )
        .bind(tenant)
        .bind(id)
        .bind(expected)
        .fetch_optional(&mut **tx)
        .await?;
        previous
            .map(|mut previous| {
                previous["enabled"] = json!(false);
                previous == json!(spec)
            })
            .unwrap_or(false)
    } else {
        false
    };
    if !disabling {
        let cfg = spec.validate(&serde_json::from_value(document)?)?;
        for (reference, kind) in std::iter::once((&cfg.model, "model"))
            .chain(cfg.tools.iter().map(|r| (r, "tool")))
            .chain(cfg.skills.iter().map(|r| (r, "skill")))
            .chain(cfg.cluster.iter().map(|r| (r, "cluster")))
            .chain(spec.compaction.iter().map(|c| (&c.provider, "compactor")))
        {
            let metadata:Option<Value>=sqlx::query_scalar("SELECT r.metadata FROM authorization_catalog c JOIN registry r ON r.id=c.entry_id AND r.version=c.entry_version WHERE c.tenant=$1 AND c.entry_id=$2 AND c.entry_version=$3 AND c.enabled FOR SHARE OF c")
            .bind(tenant).bind(&reference.id).bind(&reference.version).fetch_optional(&mut **tx).await?;
            let entry: Entry = serde_json::from_value(metadata.ok_or_else(|| {
                Error::Invalid("generation components require tenant catalog approval".into())
            })?)?;
            if entry.kind != kind {
                return Err(Error::Invalid(format!(
                    "generation component must be a {kind}"
                )));
            }
            if kind == "model" {
                let model: ModelConfig = serde_json::from_value(entry.config)?;
                let required = model.context_window as i64
                    + (model.context_window / 8).clamp(256, 4096) as i64;
                if spec.limits.tokens_per_agent < required {
                    return Err(Error::Invalid(
                        "agent token allowance is smaller than one model reservation".into(),
                    ));
                }
            }
        }
    }
    let revision: Option<i64> = if expected == 0 {
        sqlx::query_scalar("INSERT INTO generation_policies(tenant,id,revision,spec) VALUES($1,$2,1,$3) ON CONFLICT DO NOTHING RETURNING revision")
            .bind(tenant).bind(id).bind(json!(spec)).fetch_optional(&mut **tx).await?
    } else {
        sqlx::query_scalar("UPDATE generation_policies SET revision=revision+1,spec=$4 WHERE tenant=$1 AND id=$2 AND revision=$3 RETURNING revision")
            .bind(tenant).bind(id).bind(expected).bind(json!(spec)).fetch_optional(&mut **tx).await?
    };
    let revision =
        revision.ok_or_else(|| Error::Conflict("generation policy revision changed".into()))?;
    sqlx::query("INSERT INTO generation_policy_history(tenant,policy_id,revision,spec,actor) VALUES($1,$2,$3,$4,$5)")
        .bind(tenant).bind(id).bind(revision).bind(json!(spec)).bind(actor).execute(&mut **tx).await?;
    load(tx, tenant, id, false).await
}
