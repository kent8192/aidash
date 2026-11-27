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

{{- /* The standalone Preview Deployment runs one replica without an autoscaler. */ -}}
{{- define "aidash.previewReplicas" -}}
1
{{- end -}}

{{- define "aidash.databaseMaxConnections" -}}
{{- $memory := .Values.database.postgresMemoryLimit | toString -}}
{{- if not (regexMatch "^([0-9]+([.][0-9]*)?|[.][0-9]+)([eE][+-]?[0-9]+|[KMGTPE]i|[numkMGTPE])?$" $memory) -}}
  {{- fail "database.postgresMemoryLimit must be a positive Kubernetes memory quantity" -}}
{{- end -}}
{{- /* PostgreSQL's MB settings use MiB. Convert both binary and decimal Kubernetes units. */ -}}
{{- $unit := regexFind "([KMGTPE]i|[numkMGTPE])$" $memory -}}
{{- $scales := dict "Ki" 1024 "Mi" 1048576 "Gi" 1073741824 "Ti" 1099511627776 "Pi" 1125899906842624 "Ei" 1152921504606846976 "n" 0.000000001 "u" 0.000001 "m" 0.001 "k" 1000 "M" 1000000 "G" 1000000000 "T" 1000000000000 "P" 1000000000000000 "E" 1000000000000000000 -}}
{{- $bytes := mulf (trimSuffix $unit $memory | float64) (get $scales $unit | default 1) -}}
{{- $memoryMiB := divf $bytes 1048576 -}}
{{- $sharedMiB := int .Values.database.sharedBuffersMiB -}}
{{- $backendMiB := int .Values.database.activeBackendMiB -}}
{{- if or (lt $sharedMiB 1) (lt $backendMiB 1) -}}
  {{- fail "database.sharedBuffersMiB and database.activeBackendMiB must be positive" -}}
{{- end -}}
{{- /* Leave 25% of the memory limit for PostgreSQL and workload variance. */ -}}
{{- $maxConnections := divf (subf (mulf $memoryMiB 0.75) $sharedMiB) $backendMiB | int -}}
{{- if lt $maxConnections 1 -}}
  {{- fail "PostgreSQL memory limit leaves no connections after shared_buffers and 25% headroom" -}}
{{- end -}}
{{- $maxConnections -}}
{{- end -}}

{{- define "aidash.databaseConnectionBudget" -}}
{{- $replicas := add .Values.server.replicas .Values.worker.replicas -}}
{{- $gateways := 0 -}}
{{- if .Values.execution.egress.enabled -}}
  {{- $groups := merge (dict "shared" dict) .Values.execution.egress.ipGroups -}}
  {{- $gateways = mul (len $groups) (include "aidash.egressMaxReplicasPerGroup" . | int) -}}
{{- end -}}
{{- $previews := 0 -}}
{{- if and .Values.preview.enabled (not .Values.preview.inProcess) -}}
  {{- $previews = include "aidash.previewReplicas" . | int -}}
{{- end -}}
{{- /* Reserve three PostgreSQL superuser slots, twenty for jobs/commands,
      and eight connections per standalone Egress or Preview replica. */ -}}
{{- $maxConnections := include "aidash.databaseMaxConnections" . | int -}}
{{- $available := sub $maxConnections (add 3 20 (mul (add $gateways $previews) 8)) -}}
{{- /* A managed release may temporarily suspend both workloads during migration. */ -}}
{{- $budget := div $available (max 1 $replicas) -}}
{{- if lt $budget 12 -}}
  {{- fail (printf "database connection budget is %d per server/worker process (minimum 12): max_connections=%d, server+worker replicas=%d, maximum Egress replicas=%d, maximum Preview replicas=%d" $budget $maxConnections $replicas $gateways $previews) -}}
{{- end -}}
{{- $budget -}}
{{- end -}}
