#!/bin/sh
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

set -eu
# Compose mounts URL credentials as files. Kubernetes may still use secretKeyRef.
if [ -n "${AIDASH_MARKETPLACE_DATABASE_URL_FILE:-}" ]; then
  if [ -n "${AIDASH_MARKETPLACE_DATABASE_URL:-}" ]; then
    echo 'Choose one Marketplace database credential source' >&2
    exit 1
  fi
  AIDASH_MARKETPLACE_DATABASE_URL=$(cat "$AIDASH_MARKETPLACE_DATABASE_URL_FILE")
  export AIDASH_MARKETPLACE_DATABASE_URL
fi
exec aidash-marketplace-server "$@"
