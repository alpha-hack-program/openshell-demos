# TLS with Let's Encrypt (cert-manager path)

By default, the cert-manager path ([step 2a](../README.md#2a-helm-install))
gives the OpenShell gateway's Route a self-signed certificate, signed by the
chart's own CA. This doc covers the optional alternative: a
publicly-trusted, CA-signed certificate for the Route via cert-manager's
ACME support, instead of the default self-signed one.

## How it works

Setting `LETSENCRYPT_CLUSTER_ISSUER` makes the chart's `serverIssuerRef`
create a **second** server certificate signed by your ACME issuer, for the
Route FQDN specifically — the **internal** certificate stays signed by the
chart's own CA, so mTLS client auth keeps working unchanged. The gateway
serves the right one of the two depending on SNI.

Because of this split, `certManager.serverDnsNames` behaves differently
depending on whether `serverIssuerRef` is set:

- **Not set (self-signed, the default):** `serverDnsNames` feeds the single
  chart-CA-signed certificate, so the Route host is *appended* to the base
  internal-SAN list.
- **Set (Let's Encrypt):** `serverDnsNames` feeds *only* the external ACME
  certificate — the internal certificate's SANs come from the chart's own
  defaults automatically, regardless of this value. The external
  certificate's `dnsNames` are used as-is, with no filtering, so the list
  must contain **only** the externally-resolvable Route FQDN — no internal
  names (rejected by the chart's own guard), no IPs, and no bare
  single-label names (both silently accepted by the chart but rejected by
  ACME). That's why [step 2a](../README.md#2a-helm-install)'s install
  command *replaces* `serverDnsNames` wholesale in this branch instead of
  appending to it.

Also see the `OPENSHELL_NAMESPACE` naming constraint in this demo's main
README — the Route host doubles the `openshell-` prefix if the namespace
already starts with it, which can push the Let's Encrypt certificate's
CommonName over the 64-byte X.509 limit.

## Setting it up

1. Set `LETSENCRYPT_CLUSTER_ISSUER` in your `.env` to the name of an
   existing Let's Encrypt `ClusterIssuer` on your cluster (e.g.
   `letsencrypt-prod`):

   ```bash
   CERT_MANAGER=true
   LETSENCRYPT_CLUSTER_ISSUER=letsencrypt-prod
   ```

2. If you don't have a `ClusterIssuer` yet, create one. DNS-01 is
   recommended for passthrough Routes (HTTP-01 needs port 80, which
   passthrough doesn't expose). The solver configuration depends on your
   DNS provider — this example uses Route53, but cert-manager supports
   [many providers](https://cert-manager.io/docs/configuration/acme/dns01/):

   ```bash
   oc apply -f - <<'EOF'
   apiVersion: cert-manager.io/v1
   kind: ClusterIssuer
   metadata:
     name: letsencrypt-prod
   spec:
     acme:
       server: https://acme-v02.api.letsencrypt.org/directory
       email: your-email@example.com        # Let's Encrypt notifications
       privateKeySecretRef:
         name: letsencrypt-prod-account-key
       solvers:
         - dns01:
             route53:                        # replace with your DNS provider
               region: us-east-1
               # accessKeyID / secretAccessKeySecretRef or IRSA — see
               # cert-manager docs for your provider
   EOF

   # Verify the issuer is ready
   oc get clusterissuer letsencrypt-prod
   ```

3. Run [step 2a](../README.md#2a-helm-install)'s `helm upgrade --install`
   as written — it already branches on `LETSENCRYPT_CLUSTER_ISSUER` being
   set and passes `serverIssuerRef` accordingly.

## Waiting for the certificate

`helm upgrade --install` returning success only means the `Certificate`
object was *created*, not that cert-manager has finished the ACME challenge
and rotated the Route onto the real Let's Encrypt-signed cert. The
StatefulSet rollout step 2a already waits on doesn't cover this either,
since it's a separate resource with no dependency the chart declares.

If [step 2b](../README.md#2b-register-the-gateway-with-the-cli)'s mTLS
extraction runs while the Route is still serving the chart's own
self-signed cert (issuance can take a few minutes), the Let's Encrypt chain
it appends to `ca.crt` ends up empty, and every later CLI command against
this gateway fails with `invalid peer certificate: UnknownIssuer` once the
cert *does* rotate — a failure mode that's easy to hit and confusing to
diagnose after the fact, since `gateway add`/`whoami` may keep working for
a while on the stale cert before it flips. Wait for both of these before
moving on to step 2b:

```bash
oc -n "$OPENSHELL_NAMESPACE" wait --for=condition=Ready \
  certificate/openshell-server-external --timeout=300s
```

`Certificate: Ready` only confirms cert-manager finished the ACME challenge
and wrote the cert into its Secret — it doesn't guarantee the pod behind
the passthrough Route has already reloaded onto it (that's a separate
propagation step with no condition to wait on). Confirm the Route is
actually serving the Let's Encrypt cert before moving on:

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
for i in $(seq 1 30); do
  ISSUER=$(echo | openssl s_client -connect "${ROUTE_HOST}:443" \
    -servername "${ROUTE_HOST}" 2>/dev/null | openssl x509 -noout -issuer 2>/dev/null)
  echo "$ISSUER" | grep -q "Let's Encrypt" && { echo "Route is serving the Let's Encrypt cert."; break; }
  echo "Still on the old cert ($ISSUER) — waiting..."
  sleep 10
done
```

On a fresh cluster with a real ACME production issuer, this whole sequence
(the `Certificate` resource name above, `oc wait`, then the polling loop)
completes cleanly end to end, with the condition going `Ready` and the
Route already serving the Let's Encrypt-signed cert on the first poll — no
propagation delay to wait out in practice, though the wait loop above still
covers the case where there is one.
