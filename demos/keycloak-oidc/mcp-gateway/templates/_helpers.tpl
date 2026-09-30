{{- define "mcp-gateway.allowedRoutes" -}}
namespaces:
  from: Selector
  selector:
    matchExpressions:
      - key: kubernetes.io/metadata.name
        operator: In
        values:
          {{- range .Values.gateway.allowedNamespaces }}
          - {{ . }}
          {{- end }}
{{- end -}}
