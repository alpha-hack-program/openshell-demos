# Dashboards

Perses dashboards this demo publishes to the cluster, as applied.

> Not to be confused with [`../audit-dashboard/`](../audit-dashboard/) —
> that's this repo's own purpose-built Helm-deployed app showing
> `user → sandbox → MCP server` and per-turn compliance verdicts. The
> dashboards here are Perses CRs rendered into the RHOAI console.

| File | Object | Purpose |
|---|---|---|
| `maas-usage-fixed.json` | `PersesDashboard/dashboard-3-maas-usage-admin-fixed` in `redhat-ods-monitoring` | Working copy of RHOAI's stock MaaS **Usage** dashboard |

## maas-usage-fixed.json

RHOAI's stock `dashboard-3-maas-usage-admin` renders every number as 0 on
this cluster, and silently omits most users. Three independent defects,
each of which alone is enough to produce that:

1. **Counter names.** It queries `authorized_calls_total` /
   `authorized_hits_total` / `limited_calls_total`, but the Limitador
   shipped here exposes them without the `_total` suffix. Confirmed:
   `authorized_calls` → 7 series, `authorized_calls_total` → 0.
2. **Datasource.** It reads the Data Science MonitoringStack, whose
   `namespaceSelector: null` confines it to its own namespace — while
   Limitador's PodMonitor lives in `kuadrant-system`. That stack holds
   none of these counters, so even correctly-named queries return
   nothing. This copy uses `cluster-prometheus-datasource`
   (`thanos-querier.openshift-monitoring.svc:9091`), which federates
   user-workload monitoring.
3. **Users with no token metrics disappear.** Every panel masks the
   request counters against `authorized_hits` — that join is how the
   `$model` filter is applied, since `authorized_calls` has no `model`
   label. But `authorized_hits` is the *token* counter, and MaaS only
   extracts `usage` from OpenAI-chat-shaped responses. A model served
   over the Anthropic Messages route therefore has requests and no hits,
   and its users vanish from every panel rather than showing zeros. This
   copy falls back to deriving `model` from `limitador_namespace`
   (`llm-d-demo/deepseek-flash-anthropic` → `deepseek-flash-anthropic`)
   wherever no hits exist, with `unless` preserving the authoritative
   name where they do, so `alibaba/qwen3-8b` isn't also listed under its
   route name.

It is a **copy, not a patch**: the stock dashboard is owned by the
`maas.opendatahub.io` `Config` CR and reconciled by `maas-observability`,
so an in-place edit is reverted. The copy drops `ownerReferences` (which
would garbage-collect it alongside the owner) and sets
`app.kubernetes.io/managed-by: openshell-demos` so that controller won't
adopt it. The original is left untouched; both appear in the console.

### Regenerating

Don't hand-edit this file — re-derive it, so an RHOAI update to the stock
dashboard carries over:

```bash
./scripts/20-fix-maas-usage-dashboard.sh            # render + apply
./scripts/20-fix-maas-usage-dashboard.sh --dry-run  # render only
```

The script reports how many references it rewrote. If it ever reports
`0` counter references, upstream has fixed the suffix and this copy may
be redundant.

### Reading it

Panels use `increase()` over the dashboard's selected time range, so a
window with no traffic shows **0** — that is correct behaviour, not the
bug above. Widen the range or send a request before concluding it's
broken. To check the underlying data directly:

```bash
sum by (user, subscription) (increase(authorized_calls{user!=""}[24h]))   # requests
sum by (user, subscription) (increase(authorized_hits{user!=""}[24h]))    # tokens
```

### Known limitation

Token panels stay empty for Anthropic-Messages-format models, because
nothing counts those tokens — `authorized_hits` is never incremented for
them. Per-user **request** accounting is complete across both routes;
per-user **token** accounting only covers OpenAI-format models. The same
gap means `tokenRateLimits` on a subscription are effectively unenforced
for Messages-format models, which is worth raising upstream alongside the
`_total` naming mismatch.

### "Only one user shows up" is usually not a dashboard bug

Two effects compound into a token panel that looks broken:

1. The agents run on the Messages and Responses models, whose tokens are
   never counted (above). So the token panels see almost none of the
   bankers' real work — only whatever still speaks chat-completions.
2. What *does* still speak chat-completions is mostly the
   `mcp-market-news` news-generator, a background loop that regenerates a
   batch every few minutes forever. Whoever owns the key it runs with
   (`PLATFORM_OPENAI_API_KEY`, falling back to `OPENAI_API_KEY`) is billed
   for all of it.

Point that at a banker's key and that banker's totals dwarf
everyone else's — observed here at ~974k tokens for the key-owner against
~6.4k and ~2.1k for two bankers actively running scenes. The others are
present in the data, just rounded to nothing on a scale set by the
background traffic. Confirm with the raw counter before concluding a panel
is broken:

```promql
sum by (user) (authorized_calls{limitador_namespace=~".*deepseek-flash$"})
sum by (user) (authorized_hits{limitador_namespace=~".*deepseek-flash$"})
```

Set `PLATFORM_OPENAI_API_KEY` to a dedicated platform/service key (ideally
on its own `MaaSSubscription`) to keep per-banker attribution meaningful —
see the note in `.env.example`. `scripts/06-deploy-mcp-servers.sh` prints
which key it used, so a silent fallback to a banker's key is visible at
deploy time rather than later in the billing data.
