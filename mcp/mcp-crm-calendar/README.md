# mcp-crm-calendar

Servidor MCP en Rust que expone la agenda de reuniones de cada banquero.
Sigue el mismo estilo y convenciones que
[`alpha-hack-program/elegibility-engine-mcp-rs`](https://github.com/alpha-hack-program/elegibility-engine-mcp-rs)
y que los demás MCP de esta demo (`mcp-portfolio`, `mcp-market-news`,
`mcp-kyc-compliance`): `rmcp` sobre HTTP, JWT decodificado sin verificar
firma (la firma ya la valida un sidecar Envoy delante de este servicio) y
cada respuesta envuelta con `called_by`/`roles` como rastro de auditoría.

`get_upcoming_meetings` es la herramienta que resuelve el `client_id` que el
resto de MCP de la demo (`mcp-portfolio`, `mcp-kyc-compliance`, `mcp-market-news`)
necesitan como entrada para preparar una reunión — sin llamarla antes, el
agente no sabe con quién es la próxima reunión del banquero.

## Configuración

| Variable | Descripción | Por defecto |
|---|---|---|
| `DATABASE_URL` | Cadena de conexión al PostgreSQL compartido por los cuatro MCP de la demo, inyectada desde un `Secret` de Kubernetes. | *(requerida)* |
| `BIND_ADDRESS` | Dirección donde escucha el servidor HTTP. | `127.0.0.1:8004` |
| `MCP_DISABLE_HOST_CHECK` | Si es `1`/`true`, desactiva la validación de `Host` del transporte streamable-http (útil tras un proxy). | `false` |
| `MCP_ALLOWED_HOSTS` | Lista de hosts adicionales permitidos, separados por comas. | *(vacío)* |
| `MCP_STATEFUL_MODE` | Activa el modo con estado de sesión del transporte streamable-http. | `false` |

La base de datos es un servicio PostgreSQL compartido con los otros tres MCP
de la demo (`postgres.demo-banca.svc.cluster.local:5432`, por ejemplo) — no
un fichero local: los cuatro corren como pods distintos en Kubernetes.

Al arrancar, el binario ejecuta `sqlx::migrate!("./migrations")`, que aplica:

- `migrations/0001_init.sql` — el esquema completo compartido por los cuatro
  MCP (7 tablas + semillas de `bankers`/`clients`/`positions`/`performance_snapshots`).
  Es **byte-idéntico** al de `mcp-portfolio`, para que su checksum coincida
  en la tabla `_sqlx_migrations` que comparten los cuatro servicios.
- `migrations/0002_seed_meetings.sql` — las semillas de `meetings` propias de
  este servicio. Van en un fichero aparte (versión 2, no dentro de
  `0001_init.sql`) precisamente para no romper ese checksum compartido.

## Herramientas

### `get_upcoming_meetings()`

Sin parámetros. Devuelve las próximas reuniones del banquero autenticado
(`banker_id` tomado siempre del JWT, nunca de un argumento del modelo),
ordenadas por fecha ascendente:

```sql
SELECT id, client_id, datetime FROM meetings
WHERE banker_id = $1 AND datetime > now()
ORDER BY datetime ASC
```

### `get_meeting_notes(meeting_id: String)`

Valida que la reunión pertenece al banquero autenticado
(`assert_owns_meeting`) y devuelve sus notas, cliente y fecha:

```sql
SELECT notes, client_id, datetime FROM meetings WHERE id = $1
```

Si `meeting_id` no existe o pertenece a otro banquero, el error es
deliberadamente ambiguo ("meeting_id no encontrado para el llamante
autenticado") para no confirmar cuál de los dos casos es.

## Probarlo con curl

```bash
# JWT falso sin firmar (la firma la valida el sidecar Envoy, no este servicio)
PAYLOAD=$(echo -n '{"preferred_username":"bob","realm_access":{"roles":["banker"]}}' | base64 -w0 | tr '+/' '-_' | tr -d '=')
BOB_TOKEN="eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.${PAYLOAD}."

curl -s http://127.0.0.1:8004/mcp -H "Authorization: Bearer $BOB_TOKEN" -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_upcoming_meetings","arguments":{}}}'

# Debe fallar: mtg-003 es de Alice, no de Bob
curl -s http://127.0.0.1:8004/mcp -H "Authorization: Bearer $BOB_TOKEN" -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_meeting_notes","arguments":{"meeting_id":"mtg-003"}}}'
```

## Desarrollo

```bash
make build   # cargo build --release
make test    # cargo test
make lint    # cargo clippy --all-targets --all-features -- -D warnings
make fmt     # cargo fmt --all
make audit   # cargo audit
make run     # cargo run --bin mcp_server
make image   # podman build -t mcp-crm-calendar:latest -f Containerfile .
```

La imagen se construye sobre UBI9 minimal (`ubi9/rust-toolset` para
compilar, `ubi9/ubi-minimal` en runtime) y corre como usuario no-root
`1001`.
