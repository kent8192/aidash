use crate::{Error, Result};
use async_nats::ConnectOptions;

/// async-nats does not copy URL userinfo into CONNECT authentication. Share
/// decoding across event publication, UI notification and worker activation,
/// and remove credentials from the address passed into transport diagnostics.
pub(crate) fn options(url: &str, mut options: ConnectOptions) -> Result<(String, ConnectOptions)> {
	let invalid =
		|| Error::Invalid("invalid NATS broker address or authentication encoding".into());
	let mut address = reqwest::Url::parse(url).map_err(|_| invalid())?;
	if address.username().is_empty() && address.password().is_some() {
		return Err(invalid());
	}
	let decode = |value: &str| {
		percent_encoding::percent_decode_str(value)
			.decode_utf8()
			.map(|value| value.into_owned())
			.map_err(|_| invalid())
	};
	if !address.username().is_empty() {
		options = match address.password() {
			Some(password) => {
				options.user_and_password(decode(address.username())?, decode(password)?)
			}
			None => options.token(decode(address.username())?),
		};
		address.set_username("").map_err(|_| invalid())?;
		address.set_password(None).map_err(|_| invalid())?;
	}
	Ok((address.into(), options))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn transport_addresses_and_errors_do_not_disclose_url_credentials() {
		for url in [
			"nats://fixture:secret%40value@localhost:4222",
			"nats://fixture-token@localhost:4222",
		] {
			let (address, _) = options(url, ConnectOptions::new()).unwrap();
			assert_eq!(address, "nats://localhost:4222");
		}
		let error = options(
			"nats://fixture:secret%FF@localhost:4222",
			ConnectOptions::new(),
		)
		.unwrap_err()
		.to_string();
		assert!(!error.contains("secret"));
		assert!(!error.contains("fixture"));
	}
}
