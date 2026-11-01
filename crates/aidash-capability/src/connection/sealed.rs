//! Strict versioned OAuth envelopes. Secrets never implement Debug.
use super::{ClientSecretRef, secure_uri};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

fn serialize_secret<S: serde::Serializer>(v: &SecretString, s: S) -> Result<S::Ok, S::Error> {
	s.serialize_str(v.expose_secret())
}
fn serialize_optional<S: serde::Serializer>(
	v: &Option<SecretString>,
	s: S,
) -> Result<S::Ok, S::Error> {
	match v {
		Some(v) => s.serialize_some(v.expose_secret()),
		None => s.serialize_none(),
	}
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
	RegisteredClient {
		issuer: String,
		client_id: String,
		#[serde(serialize_with = "serialize_secret")]
		client_secret: SecretString,
	},
	DynamicClient {
		issuer: String,
		client_id: String,
		redirect_uri: String,
		#[serde(serialize_with = "serialize_secret")]
		client_secret: SecretString,
	},
	Connection {
		issuer: String,
		resource: String,
		client_id: String,
		client: Option<ClientSecretRef>,
		redirect_uri: Option<String>,
		#[serde(serialize_with = "serialize_secret")]
		access_token: SecretString,
		#[serde(serialize_with = "serialize_optional")]
		refresh_token: Option<SecretString>,
		expires_at: Option<String>,
		scope: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		previous: Option<String>,
	},
}
#[derive(Serialize, Deserialize)]
struct Envelope {
	v: u8,
	#[serde(flatten)]
	value: Payload,
}
#[derive(Debug, thiserror::Error)]
#[error("invalid sealed OAuth payload")]
pub struct Invalid;
pub fn seal(value: Payload) -> Result<Zeroizing<Vec<u8>>, Invalid> {
	validate(&value)?;
	let bytes = Zeroizing::new(serde_json::to_vec(&Envelope { v: 1, value }).map_err(|_| Invalid)?);
	if bytes.len() > 65536 {
		return Err(Invalid);
	}
	Ok(bytes)
}
pub fn open(bytes: &[u8]) -> Result<Payload, Invalid> {
	if bytes.is_empty() || bytes.len() > 65536 {
		return Err(Invalid);
	}
	let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| Invalid)?;
	if envelope.v != 1 {
		return Err(Invalid);
	}
	validate(&envelope.value)?;
	Ok(envelope.value)
}
fn validate(v: &Payload) -> Result<(), Invalid> {
	let (issuer, client_id) = match v {
		Payload::RegisteredClient {
			issuer, client_id, ..
		} => (issuer, client_id),
		Payload::DynamicClient {
			issuer,
			client_id,
			redirect_uri,
			..
		} => {
			if !secure_uri(redirect_uri) {
				return Err(Invalid);
			}
			(issuer, client_id)
		}
		Payload::Connection {
			issuer,
			resource,
			client_id,
			client,
			redirect_uri,
			access_token,
			previous,
			..
		} => {
			if previous.as_ref().is_some_and(|v| !super::version_valid(v))
				|| !secure_uri(resource)
				|| access_token.expose_secret().is_empty()
				|| client.as_ref().is_some_and(|c| {
					c.id.is_nil()
						|| (matches!(c.kind, super::ClientKind::Dynamic)
							&& redirect_uri.as_ref().is_none_or(|r| !secure_uri(r)))
				}) {
				return Err(Invalid);
			}
			(issuer, client_id)
		}
	};
	if !secure_uri(issuer)
		|| client_id.is_empty()
		|| client_id.len() > 4096
		|| client_id.chars().any(char::is_control)
	{
		return Err(Invalid);
	}
	Ok(())
}
