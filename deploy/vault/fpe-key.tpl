{{- with secret "secret/openobscure/fpe-key" -}}{{ .Data.data.value }}{{- end -}}
