#!/usr/bin/env bash
# Prove that the Community build works from the committed tree without
# enterprise/. docs/operations/community-and-enterprise-builds.md describes
# each step.
set -euo pipefail
root=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)
stage=$(mktemp -d "${TMPDIR:-/tmp}/aidash-community-export.XXXXXX")
settings=$(mktemp -d "${TMPDIR:-/tmp}/aidash-community-settings.XXXXXX")
descriptions=$(mktemp -d "${TMPDIR:-/tmp}/aidash-community-openapi.XXXXXX")
project=aidash-community-export
image=aidash-community-export:runtime
cleanup() {
	if [ -f "$stage/compose.yaml" ]; then
		docker compose --project-directory "$stage" -p "$project" --profile dev down -v || true
	fi
	rm -rf "$stage" "$settings" "$descriptions"
}
trap cleanup EXIT

git -C "$root" archive --format=tar HEAD | tar -x -C "$stage"
if [ ! -d "$stage/enterprise" ]; then
	echo "HEAD has no enterprise/ to remove" >&2
	exit 1
fi
rm -rf "$stage/enterprise"
# Cloud infrastructure lives inside enterprise/ after the licensing cutover
# (#254). A top-level infra/ would put an Enterprise path back in Community.
if [ -e "$stage/infra" ]; then
	echo "the Community tree still has a top-level infra/" >&2
	exit 1
fi

# The full tree builds the Community aidash-server package, so its OpenAPI
# description must match the one generated without enterprise/ byte for byte.
target="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target" AIDASH_OPENAPI_DESCRIPTION_ONLY=1 \
	AIDASH_OPENAPI_OUTPUT="$descriptions/full.json" "$root/scripts/generate-api.sh"
(
	cd "$stage"
	CARGO_TARGET_DIR="$target" AIDASH_OPENAPI_DESCRIPTION_ONLY=1 \
		AIDASH_OPENAPI_OUTPUT="$descriptions/export.json" scripts/generate-api.sh
)
if ! cmp -s "$descriptions/full.json" "$descriptions/export.json"; then
	echo "the Community and exported OpenAPI descriptions differ" >&2
	diff -u "$descriptions/full.json" "$descriptions/export.json" >&2 || true
	exit 1
fi
python3 "$root/scripts/check-community-openapi.py" "$descriptions/export.json"
cd "$stage"

cargo metadata --locked --format-version 1 | python3 scripts/check-workspace-boundaries.py
docker build --target runtime --build-arg CARGO_PROFILE=dev --tag "$image" .
# The published Community image must carry no Enterprise path and must ship
# its component notice, the license texts of its third-party material and the
# attribution notices under /usr/share/doc/aidash.
python3 "$root/scripts/check-licensing.py" artifact --image "$image"

chmod 755 "$settings"
printf '[deployment]\noffering = "cloud"\n' >"$settings/base.toml"
chmod 644 "$settings/base.toml"
if output=$(docker run --rm -v "$settings:/settings:ro" -e REINHARDT_SETTINGS_DIR=/settings "$image" serve 2>&1); then
	echo "the Community image served the cloud Offering" >&2
	exit 1
fi
if ! grep -qF 'the Community build does not run the cloud Offering' <<<"$output"; then
	echo "$output" >&2
	exit 1
fi

docker compose -p "$project" --profile dev up --build --wait backend
curl --fail --silent --show-error "http://127.0.0.1:${AIDASH_BACKEND_PORT:-18080}/health"
echo
echo "The Community build works from a tree without enterprise/."
