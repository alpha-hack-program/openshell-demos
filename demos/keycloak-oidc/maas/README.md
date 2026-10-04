# MaaS models for this demo

Two extra models published through the cluster's Models-as-a-Service
gateway, so both agents reach their LLM through MaaS rather than calling a
vendor API directly. That is what makes per-user keys meaningful: MaaS
attributes every request to the key's user, so each banker's spend is
separable instead of pooled.

| File | Serves | Needed by |
|---|---|---|
| `externalmodel-deepseek-flash-anthropic.json` + `maasmodelref-…` | Anthropic **Messages** API | Claude Code |
| `externalmodel-deepseek-flash-responses.json` + `maasmodelref-…` | OpenAI **Responses** API | Codex |

The stock `deepseek-flash` model MaaS already ships serves
chat-completions, which is what `mcp-market-news`' generator and
`session-auditor` use. Neither agent can use it: Claude Code speaks the
Messages API, and codex-cli rejects `wire_api = "chat"` outright since
0.146 and sends MCP tools as `"type": "namespace"` tools that only the
Responses API carries.

## apiFormat values

`apiFormat` is the field that selects the translation. `oc explain
externalmodel.spec.externalProviderRefs.apiFormat` documents only two:

```
- "openai-chat": OpenAI Chat Completions API (/v1/chat/completions)
- "messages":    Anthropic Messages API (/v1/messages)
```

**`openai-responses` also works and is not documented there.** It was
found from the gateway's own rejection of a wrong guess:

```
ext_proc_error ... unsupported format combination: openai-responses → responses
```

i.e. the ext_proc recognises the incoming request as `openai-responses`
and the error names the vocabulary. Since it's undocumented, treat it as
unsupported-but-working and confirm with the MaaS folks before depending
on it.

`path` is the **upstream** path on the provider, not the client-facing
one: `/responses` and `/anthropic/v1/messages` here, matching what
DeepSeek serves, while clients call `/v1/responses` and `/v1/messages` on
MaaS.

## Applying

```bash
oc apply -f externalmodel-deepseek-flash-anthropic.json
oc apply -f maasmodelref-deepseek-flash-anthropic.json
oc apply -f externalmodel-deepseek-flash-responses.json
oc apply -f maasmodelref-deepseek-flash-responses.json
```

Then add each model to a `MaaSSubscription`, or it stays `Pending` with
`GovernanceAttached=False (NoPairingFound)` and every call returns
`403 subscription ... does not include model`:

```bash
oc patch maassubscription <sub> -n models-as-a-service --type=json -p '[
  {"op":"add","path":"/spec/modelRefs/-","value":{
     "name":"deepseek-flash-responses","namespace":"llm-d-demo",
     "tokenRateLimits":[{"limit":10000,"window":"5m"}]}}
]'
```

The `MaaSModelRef` is the piece that is easy to miss: the `ExternalModel`
reconciler creates only the HTTPRoute, and nothing creates the
`MaaSModelRef` for you — without it the model never appears in the list of
models a subscription can include.

## Client base URLs

MaaS serves these under `/v1`, so clients need the `/v1` suffix even
though the upstream provider does not use it — `CODEX_BASE_URL` must end
in `/v1` or codex-cli posts to `/responses` and gets
`BadRequest - unsupported API endpoint`.

## Known limitation

Only the chat-completions model produces token metrics. `authorized_hits`
(the token-weighted counter) is never incremented for the Messages or
Responses routes, because MaaS only extracts `usage` from
OpenAI-chat-shaped responses. Per-user **request** accounting covers all
three; per-user **token** accounting covers only `deepseek-flash`. The
same gap means `tokenRateLimits` are effectively unenforced for the two
models here.
