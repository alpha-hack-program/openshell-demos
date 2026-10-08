# demo-env — inspect and snapshot the keycloak-oidc demo's local state

`source scripts/as.sh <id>` switches your shell between admin, alice, bob
and charlie by re-pointing `XDG_CONFIG_HOME`/`XDG_STATE_HOME` at that
persona's own `~/.local/state/openshell-demos/oc-<id>/{config,state}` (see
[`demos/keycloak-oidc/docs/identity-switching.md`](../../demos/keycloak-oidc/docs/identity-switching.md)).
That works, but it is write-only. Two things it can't do:

- **Tell you whether those files are in a state the guide can run from.**
  `as.sh` performs exactly one check (is the stored gateway endpoint the
  one this `.env` expects?) and otherwise hands you a shell that looks
  fine until the first `openshell` call fails for a reason that doesn't
  mention a file — `isn't a member of this workspace`,
  `invalid peer certificate: UnknownIssuer`, `No active gateway`.
- **Hold more than one set of them at a time.** The `oc-<id>` directories
  live under `$HOME` and are shared by every checkout on the machine, so
  running the guide against a second cluster silently destroys the first
  run's four browser logins. Getting them back means four more logins.

`demo-env` fixes both: `status` and `where` for the first, `save` /
`list` / `restore` for the second.

## Install

```bash
make install     # builds release and installs to ~/bin/demo-env
```

Or run it from the checkout with `cargo run -- <args>`.

## Commands

| Command | What it does |
|---|---|
| `demo-env status` | Check every file is present, valid, and points at the cluster `.env` names. Exit 1 on any failure. |
| `demo-env where` | List every path the demo depends on and whether it exists. |
| `demo-env save <slot>` | Copy the whole current state into a named slot. |
| `demo-env list` | Show saved slots, newest first. |
| `demo-env show <slot>` | One slot in detail. |
| `demo-env diff <slot>` | Compare a slot against what is on disk now. Exit 1 if they differ. |
| `demo-env restore <slot>` | Put a slot's files back, after an automatic backup. |
| `demo-env rm <slot>` | Delete a slot. |

Global flags: `--demo-dir`, `--state-root`, `--slots-dir`, `--gateway`,
`--json`, `--no-color`. `-i/--identity <id>` and `--scope
<identities\|shared\|demo>` are repeatable and narrow most commands.
Everything is read-only except `save`, `restore` and `rm`.

## What counts as "the demo's state"

Three unrelated places on disk, which is the reason a tool is warranted at
all — no single `git status` or `ls` covers them:

| Scope | Path | Holds |
|---|---|---|
| `identities` | `~/.local/state/openshell-demos/oc-<id>/{config,state}` | Per-persona gateway registration (`metadata.json`), OIDC session (`oidc_token.json`), client mTLS material (`mtls/{ca,tls}.{crt,key}`), CLI state |
| `shared` | `${XDG_CACHE_HOME:-~/.cache}/openshell-demos/keycloak-oidc-ca-bundle.pem` | The ingress-CA bundle every persona's `SSL_CERT_FILE` points at — deliberately not per-identity |
| `demo` | `demos/keycloak-oidc/` | `.env`, `keycloak/realm-export.rendered.json`, `onboarding-web-admin-session/`, `openclaw-*-session/` |

The canonical personas are admin, alice, bob and charlie; any other
`oc-*` directory is picked up automatically, so a run with extra bankers
isn't silently excluded.

## What `status` actually checks

Each check corresponds to a failure that costs time when it happens live,
and prints the fix rather than just the symptom. The ones worth knowing
about, because none of them looks like a file problem when it bites:

- **Stale cluster registration.** `metadata.json` still points at the
  cluster you ran against last week. Same comparison `use_identity` makes,
  but for every persona at once rather than the one you're switching to.
- **Wrong persona in a persona's tree.** `oc-bob/` holding alice's token
  passes every existence check. `status` decodes the access token's
  `preferred_username` claim and compares it to the directory. (`admin` is
  expected to be `openshell-admin` — that's not a mismatch.)
- **`ca.crt` missing the Route's issuing chain** on the Let's Encrypt path.
  The CLI's gRPC channel pins trust to that file and never falls back to
  the system store, so one certificate where there should be two means
  every call fails with `UnknownIssuer`.
- Missing or expired client certificates, a `tls.key` readable beyond its
  owner, an access token with no refresh token behind it, an
  `OPENSHELL_NAMESPACE` that starts with `openshell-`, and a Route host
  over the 64-byte X.509 CommonName limit on the ACME path.

An expired *access* token is reported as information, not a problem, when
a refresh token is present — the CLI rotates it on next use.

## Slots

A slot is one complete demo state. The usual shape is one per cluster:

```bash
demo-env save cluster-a --note "all four bankers logged in"
# … later, after re-running the guide against a different cluster …
demo-env list
demo-env restore cluster-a
```

On disk:

```text
~/.local/state/openshell-demos/snapshots/<slot>/
├── manifest.json                      # hashes, sizes, modes, cluster, note
├── identities/<id>/{config,state}/…   # verbatim oc-<id> tree
├── shared/keycloak-oidc-ca-bundle.pem
└── demo/{.env,keycloak/…,…}
```

Properties worth knowing:

- **The payload is a verbatim tree copy**, not just the files `status`
  understands. The openshell CLI may add state files between versions and
  a restore that quietly dropped them would be worse than none.
- **`manifest.json` carries a hash for every file**, so `list`, `show` and
  `diff` never read the payload — and so never touch the secrets in it.
- **Restore replaces directories, it does not merge them.** A leftover
  `gateways/<other-cluster>/` surviving a restore is exactly the stale
  registration this tool exists to catch.
- **Restore backs up first**, into an `autosave-<timestamp>` slot covering
  the same selection, so restoring onto a working setup is undoable. Pass
  `--no-backup` to skip it, `--dry-run` to see the plan and stop.
- **Restore only touches the top-level demo paths the slot owns** — never
  the demo directory as a whole.
- **Saves are atomic.** A slot is built in a staging directory and renamed
  into place, so an interrupted save leaves nothing `restore` could pick up.

### Secrets

A slot contains OIDC refresh tokens, a TLS private key and the demo
`.env`'s API keys. The slots directory and every slot in it are created
mode 0700, and slots live outside the repo by default. They are still
plaintext copies of credentials: don't sync the directory anywhere you
wouldn't sync `.env` itself.

### Known limitations

`diff` compares file contents, not permission bits — a `tls.key` that was
chmod'ed to 0644 shows as unchanged there. `status` catches that case.

**[VERIFY] against a real login.** The JSON field names this reads —
`gateway_endpoint` and `auth_mode` in `metadata.json`, and
`access_token`/`refresh_token`/`expires_at`/`issuer`/`client_id` in
`oidc_token.json` — were taken from `scripts/lib-use-identity.sh`,
`scripts/17-provision-scene7-both-agents.sh` and
[`util/parrot`](../parrot)'s own reader, not from a live file; the demo
state was exercised with a synthetic fixture. The readers accept
alternative spellings and degrade to "unknown" rather than failing, so a
mismatch costs a row of output and not a crash — but the first run
against a real set of logins should confirm `status` reports a registered
endpoint and a token subject rather than blanks. Everything else (paths,
PEM parsing, file modes, slot save/restore) is independent of those
schemas.

## Scripting

Every command takes `--json`, and exit codes are meaningful:

```bash
demo-env status --json | jq -r '.checks[] | select(.level=="fail")'
demo-env status --strict || echo "not ready"   # --strict also fails on warnings
demo-env diff cluster-a  || echo "state has drifted since the snapshot"
```

`restore` and `rm` prompt for confirmation, and refuse to assume yes when
stdin isn't a terminal — pass `--yes` to run them non-interactively.

## Development

```bash
make check    # fmt-check + clippy -D warnings + tests, the CI gate
make test
make install
```
