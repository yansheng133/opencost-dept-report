{{- define "cost-report.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Full name. With the release name "cost-report" this is "cost-report".
*/}}
{{- define "cost-report.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "cost-report.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "cost-report.selectorLabels" -}}
app.kubernetes.io/name: {{ include "cost-report.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "cost-report.labels" -}}
helm.sh/chart: {{ include "cost-report.chart" . }}
{{ include "cost-report.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: cost-report
{{- end }}

{{/*
Cluster-scoped names carry the release namespace so two installs cannot collide.
*/}}
{{- define "cost-report.clusterName" -}}
{{- printf "%s-%s" (include "cost-report.fullname" .) .Release.Namespace | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Image reference, always by digest. The tag is informational and never used.
*/}}
{{- define "cost-report.image" -}}
{{- $repo := required "image.repository is required" .Values.image.repository }}
{{- $digest := required "image.digest is required: cost-report pulls images by digest only (sha256:<64 hex>). The tag is informational." .Values.image.digest }}
{{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) }}
{{- fail (printf "image.digest must look like sha256:<64 lowercase hex>, got %q" $digest) }}
{{- end }}
{{- printf "%s@%s" $repo $digest }}
{{- end }}

{{/*
A department ID must be a label value and a DNS label (it names a ClusterRole).
*/}}
{{- define "cost-report.checkDepartments" -}}
{{- range .Values.config.departments }}
{{- if not (regexMatch "^[a-z0-9]([a-z0-9-]{0,40}[a-z0-9])?$" (toString .id)) }}
{{- fail (printf "config.departments: id %q must be lower-case letters, digits and '-' (it names a ClusterRole)" (toString .id)) }}
{{- end }}
{{- end }}
{{- end }}

{{/*
The backend's settings file.
*/}}
{{- define "cost-report.configJson" -}}
{{- $c := .Values.config }}
{{- $snap := "" }}
{{- if .Values.snapshotter.enabled }}
{{- $snap = printf "http://%s-snapshotter.%s.svc:80" (include "cost-report.fullname" .) .Release.Namespace }}
{{- end }}
{{- $out := dict
  "cluster" $c.cluster
  "opencostUrl" $c.opencostUrl
  "prometheusUrl" $c.prometheusUrl
  "snapshotterUrl" $snap
  "coverageJob" $c.coverageJob
  "labels" $c.labels
  "departments" $c.departments
  "systemNamespaces" $c.systemNamespaces
  "windows" $c.windows
  "defaultWindow" $c.defaultWindow
  "seriesSteps" $c.seriesSteps
  "cacheTtlSeconds" $c.cacheTtlSeconds
  "seriesTtlSeconds" $c.seriesTtlSeconds
  "seriesBudget" $c.seriesBudget
  "timeouts" $c.timeouts
  "thresholds" $c.thresholds
  "seals" (dict "enabled" .Values.seals.enabled "dir" "/seals" "daysBack" .Values.seals.daysBack)
}}
{{- toPrettyJson $out }}
{{- end }}
