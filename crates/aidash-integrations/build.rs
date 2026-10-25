fn main() -> Result<(), Box<dyn std::error::Error>> {
	if std::env::var_os("CARGO_FEATURE_AUTHENTICATION").is_none() {
		return Ok(());
	}
	let source = "proto/authentication.proto";
	println!("cargo:rerun-if-changed={source}");
	let mut protobuf = prost_build::Config::new();
	protobuf.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
	tonic_prost_build::configure()
		.build_client(true)
		.build_server(true)
		.compile_with_config(protobuf, &[source], &["proto"])?;
	// Framework consumers import transport through the Reinhardt facade.
	let generated =
		std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("aidash.authentication.v1.rs");
	let contents = std::fs::read_to_string(&generated)?;
	std::fs::write(
		generated,
		contents.replace("tonic::", "reinhardt::grpc::tonic::"),
	)?;
	Ok(())
}
