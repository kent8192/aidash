{{- define "aidash.affinity" -}}
{{- $affinity := deepCopy .Values.affinity -}}
{{- if .Values.memoryRecovery.existingClaim -}}
{{- $pod := get $affinity "podAffinity" | default dict -}}
{{- $terms := get $pod "requiredDuringSchedulingIgnoredDuringExecution" | default list -}}
{{- $term := dict "topologyKey" "kubernetes.io/hostname" "labelSelector" (dict "matchLabels" (dict "app.kubernetes.io/instance" .Release.Name "aidash.run/home-ledger" "true")) -}}
{{- $_ := set $pod "requiredDuringSchedulingIgnoredDuringExecution" (append $terms $term) -}}
{{- $_ := set $affinity "podAffinity" $pod -}}
{{- end -}}
{{- toYaml $affinity -}}
{{- end -}}
