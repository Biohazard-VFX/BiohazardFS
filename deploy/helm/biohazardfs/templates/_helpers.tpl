{{- define "biohazardfs.name" -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "biohazardfs.fullname" -}}
{{- if .Release.Name -}}
{{- printf "%s-%s" .Release.Name (include "biohazardfs.name" .) | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- include "biohazardfs.name" . -}}
{{- end -}}
{{- end -}}
