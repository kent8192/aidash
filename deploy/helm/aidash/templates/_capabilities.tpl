{{- define "aidash.sandboxNamespace" -}}
{{- .Values.execution.sandboxNamespace | default (printf "%s-sandbox" .Release.Name) -}}
{{- end -}}

{{- define "aidash.runtimeClassName" -}}
{{- .Values.execution.runtimeClass.name | default (printf "%s-runsc" .Release.Name) -}}
{{- end -}}

{{- define "aidash.capabilityDirectory" -}}/var/lib/aidash/capabilities{{- end -}}

{{- /* Objects live in a child of the claim mount: the store chmods its directory
to 0700, and a fresh volume root belongs to root, not the application UID. */ -}}
{{- define "aidash.capabilityStorage" -}}{{ include "aidash.capabilityDirectory" . }}/objects{{- end -}}

{{- /* The application's capability profile (Rust `Profile`, deny_unknown_fields).
It shares the Runner's guest limits; host_tasks is Runner-only. Operation and
install limits may not exceed maximum_seconds. */ -}}
{{- define "aidash.capabilityProfile" -}}
{{- $e := .Values.execution -}}
{{- $maximum := int $e.limits.maximum_seconds -}}
{{- $runner := dict "endpoint" (printf "http://%s-execution-runner.%s.svc.cluster.local:8949" .Release.Name .Release.Namespace) "token_env" "AIDASH_CORE_RUNNER_TOKEN" "image" $e.sandboxImage "runtime_class" (include "aidash.runtimeClassName" .) "namespace" (include "aidash.sandboxNamespace" .) -}}
{{- $profile := dict "admission" true "storage" (include "aidash.capabilityStorage" .) "operation_seconds" (min 120 $maximum) "install_seconds" (min 300 $maximum) "runner" $runner -}}
{{- if $e.egress.enabled -}}{{- $_ := set $profile "egress" (include "aidash.egressSettings" . | fromJson) -}}{{- end -}}
{{- toJson (mergeOverwrite (omit (deepCopy $e.limits) "host_tasks") $profile) -}}
{{- end -}}

{{- /* Managed Provider Credential descriptor, validated as the GCP host did. */ -}}
{{- define "aidash.providerCredentials" -}}
{{- $d := .Values.providerCredentials.settings -}}
{{- $invalid := "providerCredentials.settings must be a managed Provider Credential descriptor {fingerprint_key, store, broker, tenant_secret_broker}" -}}
{{- if or (not (kindIs "map" $d)) (ne (keys $d | sortAlpha | join ",") "broker,fingerprint_key,store,tenant_secret_broker") -}}{{ fail $invalid }}{{- end -}}
{{- $store := $d.store -}}
{{- if not (kindIs "invalid" $store) -}}
{{- if and (kindIs "map" $store) (eq (toJson $store.kind) "\"postgres\"") -}}
{{- if or (ne (keys $store | sortAlpha | join ",") "kind,master_key") (ne (toJson $store.master_key) (toJson (dict "env" "AIDASH_PROVIDER_STORE_MASTER_KEY"))) -}}{{ fail "providerCredentials.settings.store must reference AIDASH_PROVIDER_STORE_MASTER_KEY for a PostgreSQL Store" }}{{- end -}}
{{- if not (kindIs "invalid" $d.broker) -}}{{ fail "A PostgreSQL Store cannot configure a Credential Broker" }}{{- end -}}
{{- if not (kindIs "invalid" $d.tenant_secret_broker) -}}{{ fail "A PostgreSQL Store cannot configure a Tenant Secret Broker" }}{{- end -}}
{{- else -}}
{{- if or (not (kindIs "map" $store)) (ne (keys $store | sortAlpha | join ",") "byok_project_id,environment_id,kind") (ne (toJson $store.kind) "\"secret_manager\"") (not (kindIs "string" $store.byok_project_id)) (not (kindIs "string" $store.environment_id)) -}}{{ fail "providerCredentials.settings.store must be a Secret Manager Store {kind, byok_project_id, environment_id}" }}{{- end -}}
{{- if or (not (regexMatch "^[a-z][a-z0-9-]{4,28}[a-z0-9]$" $store.byok_project_id)) (not (regexMatch "^(develop|test|pr-[1-9][0-9]*)$" $store.environment_id)) -}}{{ fail "providerCredentials.settings.store names an invalid BYOK project or Environment" }}{{- end -}}
{{- end -}}
{{- if ne (toJson $d.fingerprint_key) (toJson (dict "env" "AIDASH_PROVIDER_FINGERPRINT_KEY")) -}}{{ fail "providerCredentials.settings.fingerprint_key must reference AIDASH_PROVIDER_FINGERPRINT_KEY when a Store is managed" }}{{- end -}}
{{- else if not (kindIs "invalid" $d.fingerprint_key) -}}{{ fail $invalid }}
{{- end -}}
{{- $broker := $d.broker -}}
{{- if not (kindIs "invalid" $broker) -}}
{{- if or (kindIs "invalid" $store) (not (kindIs "map" $broker)) (ne (keys $broker | sortAlpha | join ",") "audience,endpoint,issuer,kid") -}}{{ fail "providerCredentials.settings.broker requires a managed Store and exactly {endpoint, issuer, audience, kid}" }}{{- end -}}
{{- range $key, $value := $broker -}}
{{- if or (not (kindIs "string" $value)) (not $value) -}}{{ fail (printf "providerCredentials.settings.broker.%s must be a non-empty string" $key) }}{{- end -}}
{{- end -}}
{{- if ne $broker.audience $store.environment_id -}}{{ fail "providerCredentials.settings.broker.audience must be the Store's Environment" }}{{- end -}}
{{- end -}}
{{- $tenant := $d.tenant_secret_broker -}}
{{- if not (kindIs "invalid" $tenant) -}}
{{- if or (kindIs "invalid" $store) (not (kindIs "map" $tenant)) (ne (keys $tenant | sortAlpha | join ",") "audience,endpoint,issuer,kid") -}}{{ fail "providerCredentials.settings.tenant_secret_broker requires a managed Store and exactly {endpoint, issuer, audience, kid}" }}{{- end -}}
{{- range $key, $value := $tenant -}}
{{- if or (not (kindIs "string" $value)) (not $value) -}}{{ fail (printf "providerCredentials.settings.tenant_secret_broker.%s must be a non-empty string" $key) }}{{- end -}}
{{- end -}}
{{- if ne $tenant.audience (printf "%s-tsec" $store.environment_id) -}}{{ fail "providerCredentials.settings.tenant_secret_broker.audience must be the Store's Environment with a -tsec suffix" }}{{- end -}}
{{- else if or (not (kindIs "invalid" $broker)) (and (not (kindIs "invalid" $store)) (eq (toJson $store.kind) "\"secret_manager\"") .Values.execution.egress.enabled) -}}{{ fail "Cloud and Dedicated Secret Manager stores require providerCredentials.settings.tenant_secret_broker when a broker is configured or egress is enabled" }}{{- end -}}
{{- toJson (dict "provider_credentials" $d) -}}
{{- end -}}

{{- /* Managed public GCIP fragment, validated as the GCP host did. */ -}}
{{- define "aidash.gcip" -}}
{{- $s := .Values.gcip.settings -}}
{{- if or (not (kindIs "map" $s)) (ne (keys $s | join ",") "dashboard") (not (kindIs "map" $s.dashboard)) (ne (keys $s.dashboard | join ",") "gcip") (not (kindIs "map" $s.dashboard.gcip)) -}}{{ fail "gcip.settings must select GCIP only: {dashboard: {gcip: {...}}}" }}{{- end -}}
{{- $g := $s.dashboard.gcip -}}
{{- $allowed := list "project_id" "web_api_key" "public_origin" "shared_pool" "tenant_bindings" "providers" "password_sign_up" "sign_in_domains" "mfa_mandatory" "auth_helper" "session_absolute_seconds" "session_idle_seconds" -}}
{{- range $key := keys $g -}}
{{- if not (has $key $allowed) -}}{{ fail (printf "gcip.settings.dashboard.gcip.%s is not a managed GCIP setting" $key) }}{{- end -}}
{{- end -}}
{{- if and (hasKey $g "auth_helper") (not (has $g.auth_helper (list "firebase" "public_origin"))) -}}{{ fail "gcip.settings auth_helper must be firebase or public_origin" }}{{- end -}}
{{- if or (not (kindIs "string" $g.project_id)) (not $g.project_id) (not (kindIs "string" $g.web_api_key)) (not (trim $g.web_api_key)) (not (kindIs "map" $g.tenant_bindings)) -}}{{ fail "gcip.settings needs project_id, web_api_key and tenant_bindings" }}{{- end -}}
{{- if or (not .Values.deployment.endpoint) (ne (toJson $g.public_origin) (toJson .Values.deployment.endpoint)) -}}{{ fail "gcip.settings public_origin must equal deployment.endpoint" }}{{- end -}}
{{- toJson $s -}}
{{- end -}}

{{- define "aidash.egressSettings" -}}
{{- $e := .Values.execution.egress -}}
{{- if not (regexMatch "^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+$" $e.clusterIP) -}}{{ fail "execution.egress.clusterIP must be a reserved internal IPv4 Service IP" }}{{- end -}}
{{- if not .Values.execution.runner.enabled -}}{{ fail "execution.egress requires execution.runner.enabled" }}{{- end -}}
{{- toJson (dict "endpoint" (printf "http://%s:%d" $e.clusterIP (int $e.port)) "listen" (printf "0.0.0.0:%d" (int $e.port)) "namespace" .Release.Namespace "pod_selector" (dict "app.kubernetes.io/instance" .Release.Name "app.kubernetes.io/component" "server") "limits" $e.limits) -}}
{{- end -}}
