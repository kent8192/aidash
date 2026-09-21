//! Source-side discovery never falls back to the privileged legacy endpoint.
use super::super::{access::Access, catalog, identity::SubjectIdentity};
use crate::{
    Result,
    api_schema::PeerError,
    domain::qualified_agent,
    federation::{DiscoveredAgent, Discovery, Federation, Peer},
    registry::{Entry, Search},
};
use futures_util::{StreamExt, stream};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) async fn discover(
    f: &Federation,
    identity: &SubjectIdentity,
    search: &Search,
) -> Result<Discovery> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = discover_in(f, &mut access, search).await;
    access.finish(result).await
}

async fn discover_in(f: &Federation, access: &mut Access, search: &Search) -> Result<Discovery> {
    let mut query = search.clone();
    query.kind = Some("agent".into());
    let local = catalog::list_in(access, &query).await?;
    let mut result = Discovery {
        agents: local
            .into_iter()
            .map(|entity| DiscoveredAgent {
                node_id: f.config.node_id.clone(),
                entity,
            })
            .collect(),
        errors: vec![],
    };
    let mut peers = vec![];
    for peer in f.peers().await?.into_iter().filter(|peer| peer.enabled) {
        let resource = access.resource("node", &peer.node_id, json!({"remote_node":peer.node_id}));
        if !access.decide(&resource, "federation.discover").await? {
            continue;
        }
        // Configuration changes and disablement wait for this admitted read.
        // Compare the complete record to reject endpoint changes during admission.
        let current: Option<Peer> =
            sqlx::query_as("SELECT * FROM peers WHERE node_id=$1 AND enabled FOR SHARE")
                .bind(&peer.node_id)
                .fetch_optional(&mut *access.tx)
                .await?;
        if current.is_some_and(|current| {
            current.endpoint == peer.endpoint
                && current.credential_env == peer.credential_env
                && current.protocol_version == peer.protocol_version
        }) {
            peers.push(peer);
        }
    }
    let body =
        json!({"tenant":access.identity.tenant,"subject":access.identity.subject,"search":query});
    let mut responses = stream::iter(peers.into_iter().map(|peer| {
        let body = &body;
        async move {
            let response = f
                .request::<Vec<Entry>>(
                    &peer.node_id,
                    reqwest::Method::POST,
                    "/scoped/discover",
                    Some(body),
                )
                .await;
            (peer.node_id, response)
        }
    }))
    .buffer_unordered(8);
    while let Some((node, response)) = responses.next().await {
        let entries = match response {
            Ok(entries) => entries,
            Err(_) => {
                result.errors.push(PeerError {
                    node_id: node,
                    error: "authorized peer discovery unavailable".into(),
                });
                continue;
            }
        };
        // Validate a whole peer response before admitting any of its records.
        // Untrusted metadata cannot select another node's resource namespace.
        let mut ids = BTreeSet::new();
        if entries.len() > 1024
            || entries.iter().any(|entry| {
                entry.kind != "agent"
                    || crate::registry::validate(entry).is_err()
                    || !ids.insert((&entry.id, &entry.version))
            })
        {
            result.errors.push(PeerError {
                node_id: node,
                error: "invalid peer discovery response".into(),
            });
            continue;
        }
        for entity in entries.into_iter().filter(|entry| query.matches(entry)) {
            let mut resource = catalog::resource(access, &entity);
            resource.id = qualified_agent(&node, &entity.id, &entity.version);
            resource.attributes["remote_node"] = json!(node);
            if access.decide(&resource, "registry.read").await?
                && access.decide(&resource, "agent.execute").await?
            {
                result.agents.push(DiscoveredAgent {
                    node_id: node.clone(),
                    entity,
                });
            }
        }
    }
    result.agents.sort_by(|left, right| {
        (&left.node_id, &left.entity.id, &left.entity.version).cmp(&(
            &right.node_id,
            &right.entity.id,
            &right.entity.version,
        ))
    });
    result
        .errors
        .sort_by(|left, right| left.node_id.cmp(&right.node_id));
    Ok(result)
}
