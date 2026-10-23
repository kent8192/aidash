//! The compact JWS that Capability Tokens and Federation tokens share. A token
//! type differs in its claims and its `typ`, never in how it is signed or how
//! its header and signature are read.
use crate::{PublicKeys, SignError, TokenSigner};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signature;
use secrecy::SecretString;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) const MAX_TOKEN_LEN: usize = 8192;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
	pub alg: String,
	pub typ: String,
	pub kid: String,
}

/// Why a token's header or signature was refused. Callers map it to their own
/// failure codes and never retain the rejected input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
	Malformed,
	UnknownKid,
	InvalidSignature,
}

/// `kid` must be the signer's, so a token never names a key it was not signed with.
pub(crate) async fn mint(
	claims: &impl Serialize,
	typ: &str,
	kid: &str,
	signer: &dyn TokenSigner,
) -> Result<SecretString, SignError> {
	if kid != signer.kid() {
		return Err(SignError);
	}
	let header = Header {
		alg: "EdDSA".into(),
		typ: typ.into(),
		kid: kid.into(),
	};
	let input = format!("{}.{}", encode(&header)?, encode(claims)?);
	let signature = signer.sign(input.as_bytes()).await?;
	Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)).into())
}

pub(crate) fn encode(value: &impl Serialize) -> Result<String, SignError> {
	serde_json::to_vec(value)
		.map(|value| URL_SAFE_NO_PAD.encode(value))
		.map_err(|_| SignError)
}

pub(crate) fn decode<T: DeserializeOwned>(value: &str) -> Result<T, Rejection> {
	let bytes = URL_SAFE_NO_PAD
		.decode(value)
		.map_err(|_| Rejection::Malformed)?;
	serde_json::from_slice(&bytes).map_err(|_| Rejection::Malformed)
}

pub(crate) struct Parts<'a> {
	pub header: Header,
	pub payload: &'a str,
	pub signature: &'a str,
	/// Exactly what the signature covers: `<header>.<payload>`.
	pub input: &'a str,
}

/// The token's three parts, with its header read and its `alg` and `typ` checked.
pub(crate) fn split<'a>(token: &'a str, typ: &str) -> Result<Parts<'a>, Rejection> {
	if token.len() > MAX_TOKEN_LEN {
		return Err(Rejection::Malformed);
	}
	let mut parts = token.split('.');
	let (Some(header), Some(payload), Some(signature), None) =
		(parts.next(), parts.next(), parts.next(), parts.next())
	else {
		return Err(Rejection::Malformed);
	};
	let header: Header = decode(header)?;
	if header.alg != "EdDSA" || header.typ != typ {
		return Err(Rejection::Malformed);
	}
	let input = token.rsplit_once('.').ok_or(Rejection::Malformed)?.0;
	Ok(Parts {
		header,
		payload,
		signature,
		input,
	})
}

/// Checks the header, the key and the signature, and only then reads the
/// claims. Returns the header's `kid` with them.
pub(crate) fn verify<C: DeserializeOwned>(
	token: &str,
	typ: &str,
	keys: &PublicKeys,
) -> Result<(String, C), Rejection> {
	let parts = split(token, typ)?;
	let key = keys.0.get(&parts.header.kid).ok_or(Rejection::UnknownKid)?;
	let signature = URL_SAFE_NO_PAD
		.decode(parts.signature)
		.map_err(|_| Rejection::InvalidSignature)?;
	let signature = Signature::from_slice(&signature).map_err(|_| Rejection::InvalidSignature)?;
	key.verify_strict(parts.input.as_bytes(), &signature)
		.map_err(|_| Rejection::InvalidSignature)?;
	Ok((parts.header.kid, decode(parts.payload)?))
}
