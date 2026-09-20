use crate::{Error, Result};
use serde::Serialize;
use std::{env, net::SocketAddr};

#[derive(Clone)]
pub struct Config {
    pub node_id: String,
    pub endpoint: String,
    pub listen: SocketAddr,
    pub database_url: String,
    pub nats_url: String,
    pub api_token: String,
    pub web_dir: String,
    pub lease_seconds: i32,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NodeIdentity {
    pub id: String,
    pub endpoint: String,
    pub capabilities: Vec<String>,
    pub clusters: Vec<String>,
    pub protocol_version: String,
}

pub const PROTOCOL_VERSION: &str = "0.1";

impl Config {
    pub fn from_env() -> Result<Self> {
        let required =
            |key: &str| env::var(key).map_err(|_| Error::Invalid(format!("{key} is required")));
        let node_id = required("AIDASH_NODE_ID")?;
        validate_node_id(&node_id)?;
        let endpoint = required("AIDASH_ENDPOINT")?;
        validate_endpoint(&endpoint)?;
        let api_token = required("AIDASH_API_TOKEN")?;
        if api_token.len() < 16 {
            return Err(Error::Invalid(
                "AIDASH_API_TOKEN must be at least 16 characters".into(),
            ));
        }
        Ok(Self {
            node_id,
            endpoint,
            listen: env::var("AIDASH_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .map_err(|_| Error::Invalid("invalid AIDASH_LISTEN".into()))?,
            database_url: required("DATABASE_URL")?,
            nats_url: env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into()),
            api_token,
            web_dir: env::var("AIDASH_WEB_DIR").unwrap_or_else(|_| "web/dist".into()),
            lease_seconds: 30,
        })
    }
    pub fn identity(&self, clusters: Vec<String>) -> NodeIdentity {
        NodeIdentity {
            id: self.node_id.clone(),
            endpoint: self.endpoint.clone(),
            capabilities: vec![
                "agent.discovery".into(),
                "task.execute".into(),
                "workspace.share".into(),
            ],
            clusters,
            protocol_version: PROTOCOL_VERSION.into(),
        }
    }
}

pub fn validate_node_id(id: &str) -> Result<()> {
    let suffix = id.strip_prefix("aidash://").unwrap_or_default();
    if suffix.is_empty()
        || suffix.len() > 100
        || !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(Error::Invalid(
            "node id must be aidash:// followed by letters, numbers or hyphens".into(),
        ));
    }
    Ok(())
}

pub fn validate_endpoint(endpoint: &str) -> Result<()> {
    let url =
        reqwest::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Invalid(
            "endpoint requires HTTP(S) without inline credentials, query or fragment".into(),
        ));
    }
    Ok(())
}

pub fn secret(name: &str) -> Result<String> {
    if !name.starts_with("AIDASH_SECRET_")
        || !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(Error::Invalid(
            "credential references must use AIDASH_SECRET_* environment variables".into(),
        ));
    }
    env::var(name)
        .map_err(|_| Error::Invalid(format!("credential reference {name} is not configured")))
}

/// Peer bearer tokens must remain strong after environment-based rotation too.
pub fn peer_secret(name: &str) -> Result<String> {
    let value = secret(name)?;
    validate_peer_credential(&value)?;
    Ok(value)
}
pub fn validate_peer_credential(value: &str) -> Result<()> {
    if value.len() < 32
        || !value.bytes().all(|b| b.is_ascii_graphic())
        || value
            .bytes()
            .collect::<std::collections::HashSet<_>>()
            .len()
            < 8
    {
        return Err(Error::Invalid("peer credentials require at least 32 ASCII characters and 8 distinct characters; use a randomly generated token".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_credentials_reject_short_or_repeated_values() {
        for value in [
            "",
            "short",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abababababababababababababababab",
        ] {
            assert!(validate_peer_credential(value).is_err());
        }
        assert!(
            validate_peer_credential(
                "839b16e2f81e28c65ded5407c407305769b28bb14c80a05439d701368994b66c"
            )
            .is_ok()
        );
    }
}

pub(crate) fn same_secret(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(a.as_bytes());
    let b = Sha256::digest(b.as_bytes());
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
