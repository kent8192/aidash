{{/*
Every generated name is "<release>-environment-<component>"; the longest
components (postgres, activity) add 21 characters. Services must fit a
63-character DNS label, and the postgres/nats StatefulSets and the activity
CronJob must also leave 11 characters for their controller-generated
suffixes, so names stay within 52 characters and releases within 31.
*/}}
{{- define "environment.name" -}}
{{- if gt (len .Release.Name) 31 -}}{{ fail "release name is too long: Environment StatefulSet, CronJob and Service names need a release name of at most 31 characters" }}{{- end -}}
{{- printf "%s-environment" .Release.Name -}}
{{- end -}}
