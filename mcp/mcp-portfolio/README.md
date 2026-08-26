# mcp-portfolio

Servidor MCP en Rust que expone la cartera de clientes de un banquero.
Corre detrás de un sidecar Envoy que valida la firma del JWT contra
Keycloak antes de que la petición llegue aquí — este binario **no valida
firma**, solo decodifica el payload del token para extraer la identidad del
llamante (`preferred_username` → `banker_id`, `realm_access.roles` →
`roles`). Si no hay token, la identidad es `"unknown"`.

Sigue la misma estructura y convenciones que
[`alpha-hack-program/elegibility-engine-mcp-rs`](https://github.com/alpha-hack-program/elegibility-engine-mcp-rs).

## Herramientas

| Herramienta | Parámetros | Descripción |
|---|---|---|
| `list_my_clients` | — | Lista los clientes del banquero autenticado, incluidos `risk_profile`, `kyc_status`, `pep_flag` y `sector_focus`. El `banker_id` sale siempre del JWT, nunca de un argumento del modelo. |
| `get_positions` | `client_id: String` | Posiciones de un cliente. Falla si el cliente no pertenece al llamante. |
| `get_performance` | `client_id: String`, `period: String` (`MTD`\|`QTD`\|`YTD`) | TWR y benchmark TWR de un cliente para un periodo. Falla si el cliente no pertenece al llamante. |
| `get_top_client_by_aum` | — | Entre los clientes del banquero autenticado, el que tiene mayor patrimonio bajo gestión (suma de `market_value` en `positions`). |

Toda respuesta se envuelve como:

```json
{ "output": { ... }, "called_by": "bob", "roles": ["banker"] }
```

`called_by` y `roles` son el rastro de auditoría — se extraen del JWT, no
del `output` de la herramienta.

### Aislamiento entre banqueros

Cualquier herramienta que reciba `client_id` comprueba primero que el
cliente pertenece al banquero autenticado (`assert_owns_client` en
`src/common/portfolio_service.rs`). Si no pertenece, o si no existe, el
error es el mismo mensaje genérico — no revela cuál de los dos casos
ocurrió. Cada intento de acceso fuera del libro se registra con
`tracing::warn!(target: "tenant_violation", ...)`.

## Variables de entorno

| Variable | Obligatoria | Descripción |
|---|---|---|
| `DATABASE_URL` | Sí | Cadena de conexión a PostgreSQL, p. ej. `postgres://user:pass@postgres.demo-banca.svc.cluster.local:5432/demo` |
| `BIND_ADDRESS` | No | Dirección de escucha HTTP. Por defecto `127.0.0.1:8001`. |
| `RUST_LOG` | No | Nivel de logging (`debug`, `info`, `warn`, `error`). Por defecto `info`. |
| `MCP_DISABLE_HOST_CHECK` | No | `true` para desactivar la validación de `Host` (útil en pruebas locales). |
| `MCP_STATEFUL_MODE` | No | `true` para activar sesiones con estado en el transporte streamable-http. |
| `MCP_ALLOWED_HOSTS` | No | Lista separada por comas de hosts adicionales permitidos. |

## Base de datos

El esquema (compartido con los otros tres MCP de la demo: market-news,
kyc-compliance, crm-calendar) ya **no** lo aplica este binario. Vive en
`demos/keycloak-oidc/mcp-servers/templates/schema-init-configmap.yaml` y lo
aplica una única vez por `helm install`/`upgrade` el Job de
`schema-init-job.yaml` (hook `post-install,post-upgrade`). Al arrancar, este
servicio solo comprueba que `clients`, `positions` y `performance_snapshots`
existen — si no, falla rápido con un mensaje claro en vez de fallar de forma
confusa en la primera llamada a una herramienta. Ese cambio elimina el
problema que tenía el diseño anterior (cada servicio con su propia copia de
`migrations/0001_init.sql` ejecutada vía `sqlx::migrate!`): si dos copias
divergían aunque fuera en un byte, `sqlx` rechazaba el checksum de la que
arrancara segunda.

`mcp-portfolio` solo lee de `bankers`, `clients`, `positions` y
`performance_snapshots`.

## Build

```bash
make build   # cargo build --release --bin mcp_server
make test    # cargo test
make lint    # cargo clippy --all-targets -- -D warnings
make fmt     # cargo fmt --all
make audit   # cargo audit
```

## Ejecutar localmente

```bash
export DATABASE_URL=postgres://demo:demo@localhost:5432/demo
export BIND_ADDRESS=0.0.0.0:8001
export MCP_DISABLE_HOST_CHECK=true  # solo para pruebas locales sin proxy
make run
```

## Probar con curl (JWT falso sin firmar)

Igual que en el repositorio de referencia: como el servicio solo
base64-decodifica el payload (Envoy se encarga de validar la firma), basta
con un JWT sin firmar (`alg: none`) para las pruebas locales.

```bash
PAYLOAD=$(echo -n '{"preferred_username":"bob","realm_access":{"roles":["banker"]}}' | base64 -w0 | tr '+/' '-_' | tr -d '=')
BOB_TOKEN="eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.${PAYLOAD}."

# Inicializar sesión MCP
curl -s http://127.0.0.1:8001/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{
    "jsonrpc": "2.0", "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2025-03-26",
      "capabilities": {},
      "clientInfo": {"name": "curl-test", "version": "1.0"}
    }
  }'

# Debe funcionar: cli-001 es de Bob
curl -s http://127.0.0.1:8001/mcp \
  -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_positions","arguments":{"client_id":"cli-001"}}}'

# Debe fallar: cli-004 es de Alice, no de Bob
curl -s http://127.0.0.1:8001/mcp \
  -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_positions","arguments":{"client_id":"cli-004"}}}'
```

## Contenedor

```bash
podman build -t mcp-portfolio:latest -f Containerfile .
podman run --rm -p 8001:8001 \
  -e DATABASE_URL=postgres://demo:demo@postgres:5432/demo \
  mcp-portfolio:latest
```

Imagen UBI9 minimal, build multi-stage, usuario no-root `1001`.

### Publicación automática (GitHub Actions)

- **CI** (`.github/workflows/ci-mcp-portfolio.yml`): en cada push/PR que toque
  `mcp/mcp-portfolio/**`, corre `make check` (fmt + clippy) y `make test`.
- **Release** (`.github/workflows/release-mcp-portfolio.yml`): al empujar un
  tag `mcp-portfolio-v*`, repite el check y publica la imagen en
  `quay.io/atarazana/mcp-portfolio` — el mismo registro que ya usan
  `elegibility-engine-mcp-rs` y `compatibility-engine-mcp-rs` en
  `demos/keycloak-oidc/mcp-servers/values.yaml` — con dos tags: la versión
  sin el prefijo del tag de git (p. ej. tag `mcp-portfolio-v0.1.0` → imagen
  `0.1.0`) y `latest`. **Requiere los secretos de repo `REGISTRY_USER` y
  `REGISTRY_PASSWORD`** (credenciales de `quay.io/atarazana`), igual que la
  CI de `elegibility-engine-mcp-rs`. Configúralos en *Settings → Secrets and
  variables → Actions* antes de empujar el primer tag — no existen todavía
  en este repo.

```bash
git tag mcp-portfolio-v0.1.0
git push origin mcp-portfolio-v0.1.0
```
