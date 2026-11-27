{{- /* The kind of the disk-holding server, worker and Runner. GKE's Stateful HA
fails StatefulSets over to another zone (ADR 0067). */ -}}
{{- define "aidash.workloadKind" -}}
{{- $kind := .Values.workloadKind | default "Deployment" -}}
{{- if not (has $kind (list "Deployment" "StatefulSet")) -}}
{{- fail "workloadKind must be Deployment or StatefulSet" -}}
{{- end -}}
{{- if and .Values.highAvailability.enabled (ne $kind "StatefulSet") -}}
{{- fail "highAvailability requires workloadKind StatefulSet" -}}
{{- end -}}
{{- $kind -}}
{{- end -}}

{{- /* Rollout fields: a StatefulSet replaces its one Pod in place, which an RWO
claim needs; `deployment` holds the Deployment's own strategy. */ -}}
{{- define "aidash.workloadRollout" -}}
{{- if eq .kind "StatefulSet" -}}
serviceName: {{ .name }}
podManagementPolicy: Parallel
updateStrategy: { type: RollingUpdate }
{{- else -}}
{{ .deployment }}
{{- end -}}
{{- end -}}

{{- /* Stateful HA force-deletes a StatefulSet's Pod once its Cluster Node is
unreachable and force-attaches its regional disks on the node it moves to. */ -}}
{{- define "aidash.highAvailability" -}}
apiVersion: ha.gke.io/v1
kind: HighAvailabilityApplication
metadata:
  name: {{ .name }}
spec:
  resourceSelection:
    resourceKind: StatefulSet
  policy:
    storageSettings:
      requireRegionalStorage: true
    failoverSettings:
      forceDeleteStrategy: AfterNodeUnreachable
      afterNodeUnreachable:
        afterNodeUnreachableSeconds: {{ .seconds | int }}
{{- end -}}

{{- /* Count the maximum across every group, including the default shared one. */ -}}
{{- define "aidash.egressMaxReplicasPerGroup" -}}
10
{{- end -}}

{{- define "aidash.databaseConnectionBudget" -}}
{{- $replicas := add .Values.server.replicas .Values.worker.replicas -}}
{{- $gateways := 0 -}}
{{- if .Values.execution.egress.enabled -}}
  {{- $groups := merge (dict "shared" dict) .Values.execution.egress.ipGroups -}}
  {{- $gateways = mul (len $groups) (include "aidash.egressMaxReplicasPerGroup" . | int) -}}
{{- end -}}
{{- /* Reserve three PostgreSQL superuser slots, twenty for jobs/commands,
      and the eight connections allocated to each standalone Egress replica. */ -}}
{{- $available := sub .Values.database.maxConnections (add 3 20 (mul $gateways 8)) -}}
{{- /* A managed release may temporarily suspend both workloads during migration. */ -}}
{{- $budget := div $available (max 1 $replicas) -}}
{{- if lt $budget 12 -}}
  {{- fail (printf "database connection budget is %d per server/worker process (minimum 12): max_connections=%d, server+worker replicas=%d, maximum Egress replicas=%d" $budget (int .Values.database.maxConnections) $replicas $gateways) -}}
{{- end -}}
{{- $budget -}}
{{- end -}}
