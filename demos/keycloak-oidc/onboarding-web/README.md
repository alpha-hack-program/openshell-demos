# onboarding-web (Helm chart)

Deploys [`onboarding-web`](../../../util/onboarding-web/), the self-service
token-attach app, as a single-replica Deployment + Service + Route in this
demo's namespace. See
[`demos/keycloak-oidc/README.md`, Step 3b](../README.md#step-3b--self-service-alternative-onboarding-web)
for the full walkthrough and
[`docs/self-service-onboarding.md`](../docs/self-service-onboarding.md)
for the design rationale (Option B). This file covers the chart itself.

## What it deploys

- **Deployment** (`templates/deployment.yaml`) — one replica, no Envoy
  sidecar (this pod *is* the trusted party — it holds a standing
  Platform-Admin-equivalent `openshell` CLI session, not something gated
  behind it). An initContainer unpacks the `admin-session.tar.gz` from the
  `adminSessionSecretName` Secret into an `emptyDir` the app container reads
  from (`OPENSHELL_ADMIN_XDG_CONFIG_HOME`/`_STATE_HOME`).
- **Service** + **Route** (`templates/service.yaml`, `templates/route.yaml`)
  — exposes the app at `route.host` over HTTPS.
- **ServiceAccount** (`templates/serviceaccount.yaml`) — no extra cluster
  RBAC is granted; the pod's trust comes from the mounted admin session, not
  from Kubernetes permissions.

## Prerequisites

Both must already exist before installing this chart:

1. The `openshell-onboarding-web` Keycloak client (public, PKCE-required)
   and `openshell-onboarding-svc` user, declared in
   `../keycloak/realm-export.json` and imported in
   [step 1c](../README.md#1c-import-the-realm-json).
2. The `onboarding-web-admin-session` Secret, produced by running
   [`../scripts/10-bootstrap-onboarding-web-admin.sh`](../scripts/10-bootstrap-onboarding-web-admin.sh)
   and `oc create secret generic onboarding-web-admin-session
   --from-file=admin-session.tar.gz=...` — see Step 3b in the demo README
   for the exact commands.

## Install

Use [`../scripts/11-deploy-onboarding-web.sh`](../scripts/11-deploy-onboarding-web.sh)
rather than calling `helm` directly — it validates the prerequisites above
and waits on the rollout. Equivalent manual command:

```bash
helm upgrade --install onboarding-web ./demos/keycloak-oidc/onboarding-web \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set "route.host=${ONBOARDING_WEB_ROUTE_HOST}" \
  --set "keycloak.host=${KEYCLOAK_HOST}" \
  --set "keycloak.realm=${KEYCLOAK_REALM}"
```

## Values reference

| Value | Default | Notes |
|---|---|---|
| `image.repository` | `ghcr.io/alpha-hack-program/openshell-demos/onboarding-web` | |
| `image.tag` | `latest` | Pin to a released tag for anything beyond a quick demo |
| `route.host` | *(required)* | Must exactly match the `redirectUris` host on the `openshell-onboarding-web` Keycloak client — Keycloak rejects any mismatch. Convention: `onboarding-web-<namespace>.<apps-domain>` |
| `keycloak.host` | *(required)* | e.g. `keycloak.apps.<cluster-domain>` |
| `keycloak.realm` | `openshell` | |
| `keycloak.clientId` | `openshell-onboarding-web` | Public client — see the design doc for why it can't be confidential |
| `adminSessionSecretName` | `onboarding-web-admin-session` | Produced by `10-bootstrap-onboarding-web-admin.sh` |
| `sessionTtlSecs` | `600` | |
| `resources` | `{}` | |

## Known limitation

The admin session lives in an `emptyDir` unpacked from a Secret, not a
PVC — a Secret-backed volume doesn't write refreshed tokens back to the
Secret object, so if the mounted refresh token rotates on use and the pod
then restarts, that session goes stale and step 2 (bootstrap) must be
re-run. **[VERIFY]** whether this realm's offline refresh tokens actually
rotate on use at all — see the last item in the design doc's
[Open questions](../docs/self-service-onboarding.md#open-questions).
