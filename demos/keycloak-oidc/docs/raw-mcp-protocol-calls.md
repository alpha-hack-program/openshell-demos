# Raw MCP protocol calls (curl, for scripting/CI)

The same tool calls exercised by [step 5](../README.md#5-run-the-demo)'s
scenes, issued directly as JSON-RPC over curl — no LLM in the loop. Treat
these as **preliminary/raw-protocol checks**, not the real demo: they're how
you validate a server's wire-level behavior quickly and deterministically
(this is what `08-verify-isolation.sh` below does) before ever pointing an
agent at it. They're useful for scripting, CI, and fast iteration, since
nothing here decides *which* tool to call or *in what order* — that's
hardcoded instead of left to a model.

**The actual demo — the thing to run and to trust as end-to-end
verification — is [step 5](../README.md#5-run-the-demo)'s scenes, driven by
a real agent** making its own multi-hop tool-calling decisions against these
same servers: **Claude Code**, the preferred and primary recipe used
throughout the main guide, with **Codex** available as an optional
alternate agent (see [step 6 — Alternative
agents](../README.md#6-alternative-agents)) for exercising the identical
RBAC boundary through a different agentic harness. A curl call proving a
server returns the right JSON-RPC error is necessary but not sufficient —
it says nothing about whether an agent given only a natural-language ask
actually invokes the right tool, with the right arguments, and reports the
result (or the denial) faithfully. That agent-level behavior is exactly
what [step 5](../README.md#5-run-the-demo)'s scenes and [step 6 —
Alternative agents](../README.md#6-alternative-agents) verify, and what
curl alone cannot.

**Who runs this:** every command block below runs from **Terminal A —
admin**, using `--workspace <id>` to target each banker's sandbox — the
same admin-runs-everything-via-`--workspace` convention the main guide used
before [step 5](../README.md#5-run-the-demo) introduced per-banker
terminals. Running these from each banker's own terminal instead (B/C/D,
matching the Claude Code scenes above) works identically — `sandbox exec`
is self-service within a banker's own workspace.

## Alice: the one extra permission

Alice's book is small (just Elena Duarte), but she's the only banker who
can reach `mcp-compatibility` — the platform has to get this right for a
low-traffic user with an unusual second permission just as reliably as for
Bob's much busier book:

```bash
MCP_URL="http://mcp-compatibility.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n claude-alice --workspace alice --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'

openshell sandbox exec -n claude-alice --workspace alice --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"calc_tax\",\"arguments\":{\"income\":\"90000\"}}}" \
    "$MCP_URL"'
# Expected: 200 both times — Alice holds compatibility-user via the
# compatibility-users group; nobody else in this demo does.
```

## Bob: biggest client, meeting prep, performance diagnosis

Bob's book is the largest and most varied — this is where the multi-hop
work happens. First, who's his biggest client by AUM:

```bash
MCP_URL="http://mcp-portfolio.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_top_client_by_aum\",\"arguments\":{}}}" \
    "$MCP_URL"'
# Expected: 200 — Clara Fontán (cli-001), highest combined market_value
# across her positions in Bob's book.
```

Then meeting prep — resolve the next meeting via `mcp-crm-calendar`, then
pull that client's notes:

```bash
CRM_URL="http://mcp-crm-calendar.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${CRM_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${CRM_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_upcoming_meetings\",\"arguments\":{}}}" \
    "$MCP_URL"'
# Expected: 200 — Bob's own meetings only (mtg-001 with Clara Fontán,
# mtg-002 with Grupo Delta Textil), but ONLY whichever of those two still
# lie in the future relative to when you run this — the seed data uses
# fixed timestamps (mtg-001 is 2026-08-24T10:00:00Z), not dates relative to
# "now" (they do self-heal daily via a CronJob — see the main guide's
# step 5 setup). Running this right before a refresh returns only mtg-002.
```

Finally, performance diagnosis: Grupo Delta Textil's MTD return (`perf-002`)
is -3.4% against a +1.5% benchmark — a real underperformance worth
explaining before the meeting, not after. `get_performance` surfaces the
number; `get_relevant_news` (filtered by that client's sector) is how the
agent correlates it with an actual market event instead of guessing:

```bash
NEWS_URL="http://mcp-market-news.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${NEWS_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'

openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${NEWS_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_relevant_news\",\"arguments\":{\"tickers\":[],\"sectors\":[\"textile\"]}}}" \
    "$MCP_URL"'
# Expected: 200 — public news, no per-client isolation on this server, but
# still requires mcp-market-news-user (composited into banker).
```

## Bob probes the boundary

With a promotion decision looming and his numbers looking thin next to
Alice's and Charlie's, Bob tries to look at their books. Two different
mechanisms have to both hold for this to fail safely:

```bash
# Role-based (Envoy rbac filter) — Bob legitimately lacks compatibility-user
COMPAT_URL="http://mcp-compatibility.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"
openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${COMPAT_URL}" \
  -- bash -c 'curl -so /dev/null -w "%{http_code}" -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'
# Expected: 403 — valid token, but Bob lacks compatibility-user entirely.

# Tenant-based (mcp-portfolio's assert_owns_client) — Bob legitimately
# holds mcp-portfolio-user, so this reaches the app; the app itself has to
# refuse. cli-004 is Alice's Elena Duarte. Expected: HTTP 200 with a
# JSON-RPC-level error (code -32602).
PORTFOLIO_URL="http://mcp-portfolio.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"
openshell sandbox exec -n claude-bob --workspace bob --env "MCP_URL=${PORTFOLIO_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_positions\",\"arguments\":{\"client_id\":\"cli-004\"}}}" \
    "$MCP_URL"'
# Expected: the same deliberately-ambiguous "client_id no encontrado para
# el llamante autenticado" error Bob would get for a client_id that
# doesn't exist at all — never Elena Duarte's actual positions.
```

## Charlie: KYC-aware reasoning

Charlie's one client, Fundación Iris, carries a pending KYC review and a
PEP flag. Two servers back this up with real data: `mcp-portfolio`'s
`list_my_clients` surfaces the flags themselves (also requires
mcp-portfolio-v0.1.4+ — 0.1.3 returns id/name only); `mcp-kyc-compliance`
is the dedicated tool — it can look up the flags directly
(`get_risk_profile`) and, more importantly, search the actual regulatory
text and cite the clause instead of giving a flat yes/no
(`search_regulatory_guidance`):

```bash
MCP_URL="http://mcp-kyc-compliance.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n claude-charlie --workspace charlie --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'

openshell sandbox exec -n claude-charlie --workspace charlie --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_risk_profile\",\"arguments\":{\"client_id\":\"cli-005\"}}}" \
    "$MCP_URL"'
# Expected: 200 — Fundación Iris, kyc_status "pending", pep_flag true.

openshell sandbox exec -n claude-charlie --workspace charlie --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"search_regulatory_guidance\",\"arguments\":{\"query\":\"What approval is required before a PEP client transaction can proceed?\"}}}" \
    "$MCP_URL"'
# Expected: 200 — a fragment from the (fictional) corpus's PEP doc: prior
# compliance-officer approval plus a documented source-of-funds review,
# with the source document named — Charlie can cite the rule, not just
# assert an answer.

openshell sandbox exec -n claude-charlie --workspace charlie --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"check_suitability\",\"arguments\":{\"client_id\":\"cli-005\",\"product_id\":\"prod-002\"}}}" \
    "$MCP_URL"'
# Expected: 200 — potentially_suitable: false. prod-002 ("Meridian Balanced
# Growth Fund") is rated moderate; Fundación Iris is conservative, so
# risk_ok is false regardless of sector concentration. Try prod-001
# ("Meridian Capital Preservation Note", conservative, no sector) instead
# for a potentially_suitable: true result — see
# mcp-servers/templates/schema-init-configmap.yaml for the full 6-product
# catalog and which client/product pairs exercise which branch (risk vs.
# sector-concentration rejection).
```

Alternatively, run the isolation verification script to test every
banker/server combination — including Bob's boundary probe — automatically:

```bash
./scripts/08-verify-isolation.sh
```

Expected output:

```
PASS  alice → mcp-compatibility (calc_tax)  HTTP 200 (expected 200)
PASS  alice → mcp-portfolio (list_my_clients)  HTTP 200 (expected 200)
PASS  alice → mcp-crm-calendar (get_upcoming_meetings)  HTTP 200 (expected 200)
PASS  alice → mcp-market-news (get_relevant_news)  HTTP 200 (expected 200)
PASS  alice → mcp-kyc-compliance (get_risk_profile)  HTTP 200 (expected 200)
PASS  bob → mcp-compatibility  HTTP 403 (expected 403)
PASS  bob → mcp-portfolio (list_my_clients)  HTTP 200 (expected 200)
PASS  bob → mcp-crm-calendar (get_upcoming_meetings)  HTTP 200 (expected 200)
PASS  bob → mcp-market-news (get_relevant_news)  HTTP 200 (expected 200)
PASS  bob → mcp-kyc-compliance (get_risk_profile)  HTTP 200 (expected 200)
PASS  charlie → mcp-compatibility  HTTP 403 (expected 403)
PASS  charlie → mcp-portfolio (list_my_clients)  HTTP 200 (expected 200)
PASS  charlie → mcp-crm-calendar (get_upcoming_meetings)  HTTP 200 (expected 200)
PASS  charlie → mcp-market-news (get_relevant_news)  HTTP 200 (expected 200)
PASS  charlie → mcp-kyc-compliance (get_risk_profile)  HTTP 200 (expected 200)
PASS  bob probing cli-004 (Alice's Elena Duarte) via mcp-portfolio.get_positions — denied, no cross-tenant data leaked
PASS  bob probing cli-005 (Charlie's Fundación Iris) via mcp-portfolio.get_positions — denied, no cross-tenant data leaked
PASS  bob probing cli-004 (Alice's Elena Duarte) via mcp-kyc-compliance.get_risk_profile — denied, no cross-tenant data leaked
PASS  bob probing cli-005 (Charlie's Fundación Iris) via mcp-kyc-compliance.get_risk_profile — denied, no cross-tenant data leaked

Results: 19 passed, 0 failed
```

> For alternate ways to exercise this same RBAC boundary through a real
> coding agent instead of raw `curl`, see [step 5](../README.md#5-run-the-demo)
> (Claude Code, the primary recipe) or [step 6 — Alternative
> agents](../README.md#6-alternative-agents) (Codex).
