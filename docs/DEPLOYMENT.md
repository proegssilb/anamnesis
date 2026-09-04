# Deploying Anamnesis

This document is the operational contract: what Anamnesis needs to run, what
it promises in return, and the three supported ways to install it. It assumes
no knowledge of the source.

Everything the README describes is a *development* setup — it turns on
`ANAMNESIS_DEV_AUTH_BYPASS`, which authenticates every request as a fixed
local user. Nothing on this page does that.

---

## 1. The deployable unit

**One binary and one writable directory.** That is the whole story.

Templates, static assets (htmx, SortableJS, the stylesheet), the IANA time
zone database, and *both* sets of SQL migrations are compiled into the
binary. There is no asset bundle to ship alongside it, no `tzdata` package to
install, and no separate migration step to run — `anamnesis-web` migrates the
database itself, at startup, before it binds its socket.

| Needs | Doesn't need |
|---|---|
| A writable state directory | A web server to serve static files |
| A database (SQLite file or Postgres) | `sqlx-cli` or any migration tool |
| Outbound HTTPS to the identity provider | A system `tzdata` |
| A TLS-terminating reverse proxy | `DATABASE_URL` at *build* time |

The state directory holds file attachments (`ANAMNESIS_BLOB_ROOT`) and, if you
use SQLite, the database file. Both live under `/var/lib/anamnesis` in every
example here.

---

## 2. Configuration

All configuration is environment variables, validated once at startup. A
missing or malformed value is a loud failure naming the variable, not a
degraded run — including inside a container.

### Required

| Variable | What it is |
|---|---|
| `ANAMNESIS_DATABASE_URL` | `postgres://…` or `sqlite://…` — see §3 |
| `ANAMNESIS_BASE_URL` | The **external** URL users type, e.g. `https://anamnesis.example.com` |
| `ANAMNESIS_SESSION_SECRET` | ≥64 bytes, stable forever — see below |
| `ANAMNESIS_TIMEZONE` | An IANA zone, e.g. `America/New_York` |
| `ANAMNESIS_BOOTSTRAP_ADMIN` | The user id granted System Admin on first boot |
| `ANAMNESIS_OIDC_ISSUER_URL` | Your identity provider's issuer URL |
| `ANAMNESIS_OIDC_CLIENT_ID` | |
| `ANAMNESIS_OIDC_CLIENT_SECRET` | |

### Optional

| Variable | Default | Notes |
|---|---|---|
| `ANAMNESIS_BIND_ADDR` | `127.0.0.1:8080` | Use `0.0.0.0:8080` in a container |
| `ANAMNESIS_BLOB_ROOT` | `./blobs` | Set it explicitly; the container image already does |
| `ANAMNESIS_MAX_BODY_BYTES` | `41943040` (40 MiB) | Whole-request limit — see §5 |
| `ANAMNESIS_TLS_CA_BUNDLE` | unset | PEM bundle of extra roots for the IdP — see §6 |
| `ANAMNESIS_OIDC_SCOPES` | `openid profile email` | |
| `RUST_LOG` | `info` | |

### Three rules that bite

**`ANAMNESIS_SESSION_SECRET` must be stable across restarts.** It signs
session cookies; changing it logs every user out. Generate it **once**:

```sh
openssl rand -base64 64 | tr -d '\n'
```

`just run` generates a fresh random one on every start. That is fine for
development and a trap for anything that templates a config file — a
"regenerate on deploy" step silently logs out your whole userbase.

**`ANAMNESIS_BASE_URL` must be the external URL** — the one a browser sees,
not the container's internal address. It has to match the redirect URI
registered with your identity provider *exactly*, and its scheme alone
decides whether session cookies get the `Secure` attribute: `https://` sets
it, anything else does not.

**`ANAMNESIS_DEV_AUTH_BYPASS` must be unset.** With it on, anything that can
open a TCP connection to the port is logged in as an administrator, with no
identity provider involved at all. It logs a warning at startup for exactly
this reason. It is deliberately absent from every example file here.

`ANAMNESIS_SESSION_SECRET` and `ANAMNESIS_OIDC_CLIENT_SECRET` are real
secrets. Keep them out of version control, and out of container `--env` flags
on shared hosts (where they are visible in the process list); use an env file
with restrictive permissions, or your platform's secret store.

---

## 3. Choosing a database

Anamnesis runs the same schema on both backends, migrated at startup. Pick by
what you already operate:

- **SQLite** — the default for the Compose stack. One file in the state
  directory, nothing else to run or back up separately. The right choice for
  a single-node homelab install.
- **Postgres** — the default for a VM. Worth it when you already run a
  Postgres server, want its backup tooling, or want the database on different
  storage from the app.

Switching is a one-line change to `ANAMNESIS_DATABASE_URL`:

```sh
ANAMNESIS_DATABASE_URL=postgres://anamnesis:…@localhost/anamnesis?sslmode=verify-full
ANAMNESIS_DATABASE_URL=sqlite:///var/lib/anamnesis/anamnesis.db?mode=rwc
```

**Switching backends is not a data migration.** The new database starts
empty. There is no built-in export/import between the two.

Two syntax traps:

- **`sqlite://` takes three slashes for an absolute path.** `sqlite://foo.db`
  is *relative to the working directory*;
  `sqlite:///var/lib/anamnesis/anamnesis.db` is the absolute path. This is
  the single most commonly mistyped value in this document.
- **`?mode=rwc`** is what lets SQLite create the file on first boot. Without
  it, a fresh install fails on a database that does not exist yet.

For Postgres over TLS with a private CA, put the CA in the URL —
`?sslmode=verify-full&sslrootcert=/etc/anamnesis/ca.crt` — or set
`PGSSLROOTCERT`. This is read by the Postgres driver and is unrelated to
`ANAMNESIS_TLS_CA_BUNDLE`, which is only for the identity provider.
`sslmode=verify-full` is the only setting that actually authenticates the
server.

---

## 4. Reverse proxy requirements

Anamnesis speaks plain HTTP and does not terminate TLS. Put a proxy in front
of it. `deploy/Caddyfile.example` is a working configuration; any proxy works
if it satisfies three invariants:

1. **The public URL must equal `ANAMNESIS_BASE_URL`**, scheme included.
2. **The proxy's request body limit must be ≥ `ANAMNESIS_MAX_BODY_BYTES`.**
   Otherwise the proxy rejects large uploads first, and users see the proxy's
   error page instead of the app's message. The two numbers move together.
3. **Do not rewrite paths.** Anamnesis generates absolute paths from the root
   and cannot be hosted under a subpath.

Bind Anamnesis to `127.0.0.1` (a VM) or leave it unpublished on the Compose
network (a container), so the proxy is the only route in and no firewall rule
is load-bearing.

---

## 5. Upload limits

`ANAMNESIS_MAX_BODY_BYTES` — 40 MiB (`41943040`) by default — is the **only**
ceiling on an upload. There is no separate per-attachment cap, so this one
number is what limits attachment size, and raising it raises that. Over-limit
uploads are answered with `413 Payload Too Large`.

It is a plain byte count (`41943040`, not `40MB`).
Uploads are buffered in memory before being written to the blob store, so
peak memory is roughly *limit × concurrent uploads*: at the default, a
container memory limit below ~256 MiB is asking for an OOM kill under a
handful of simultaneous uploads.

**Watch the units when you set the proxy's matching limit.** Most proxies read
`MB` as decimal, so a literal `40MB` is 40,000,000 — 1.9 MB *below* the app's
ceiling, quietly making the proxy the real limit. Use the binary unit
(`40MiB` in Caddy, `42m` in nginx) or a plain byte count. `deploy/Caddyfile.example`
does this correctly; it is the easiest thing on this page to get subtly wrong.

---

## 6. Private certificate authorities

Anamnesis trusts a **bundled** copy of the public root certificate store and
reads nothing from the system trust store. `SSL_CERT_FILE`,
`update-ca-certificates`, and `/etc/ssl/certs` therefore have **no effect** on
it. This matters for a homelab identity provider behind an internal CA.

Point `ANAMNESIS_TLS_CA_BUNDLE` at a PEM bundle to trust those roots *in
addition to* the built-in ones:

```sh
ANAMNESIS_TLS_CA_BUNDLE=/etc/anamnesis/ca.crt
```

It fails at startup, naming the path, if the file is unreadable or contains
no PEM certificates — so a path typo or a DER-encoded file is a loud error
rather than a silently empty trust list.

This affects the **identity provider only**. Postgres has its own setting
(§3).

---

## 7. Install path A — Compose

Backed by SQLite, with Caddy terminating TLS. Requires Podman with
`podman-compose`, or Docker with Compose v2.

```sh
git clone https://github.com/proegssilb/anamnesis.git
cd anamnesis
cp deploy/compose.env.example .env
$EDITOR .env                      # fill in every CHANGEME
$EDITOR deploy/Caddyfile.example  # set your hostname
podman-compose up -d              # or: just compose-up
```

`.env` is git-ignored. The app is not published to the host — Caddy is the
only thing listening on 80/443.

For Postgres instead of SQLite:

```sh
podman-compose -f compose.yaml -f compose.postgres.yaml up -d
```

### Rootless Podman notes

- **Your subuid range must cover uid 65532**, which the image runs as. Check
  with `grep $USER /etc/subuid`: a range of 65536 is enough, and the modern
  default (`524288:65536`) is fine. A truncated range makes the container
  fail to start in a way that looks nothing like a permissions problem.
- **Healthchecks run as transient systemd user timers**, not by a daemon. If
  `podman inspect --format '{{.State.Health.Status}}'` never populates, check
  `systemctl --user is-system-running` before suspecting the image;
  `podman healthcheck run <container>` runs the probe directly and sidesteps
  the timer entirely.
- **Build with `--format docker`** if you build the image yourself. Podman's
  default OCI format has no healthcheck field, so an OCI build warns and
  silently drops the `HEALTHCHECK`. `just image` does this for you.

---

## 8. Install path B — VM with systemd

Backed by Postgres, switchable to SQLite by editing one line.

```sh
# 1. Binary (from the GitHub Release — no Rust toolchain needed)
curl -fsSL -o anamnesis-web \
  https://github.com/proegssilb/anamnesis/releases/latest/download/anamnesis-web-x86_64-unknown-linux-gnu
install -m 0755 anamnesis-web /usr/local/bin/anamnesis-web

# 2. User
useradd --system --home-dir /var/lib/anamnesis --shell /usr/sbin/nologin anamnesis

# 3. Configuration (holds two secrets — hence 0640 root:anamnesis)
install -d -m 0755 /etc/anamnesis
install -m 0640 -o root -g anamnesis \
  deploy/systemd/anamnesis.env.example /etc/anamnesis/anamnesis.env
$EDITOR /etc/anamnesis/anamnesis.env

# 4. Service
install -m 0644 deploy/systemd/anamnesis.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now anamnesis
systemctl status anamnesis
```

`StateDirectory=anamnesis` creates and owns `/var/lib/anamnesis` for you.
The unit is hardened (`ProtectSystem=strict`, an empty `CapabilityBoundingSet`,
`SystemCallFilter=@system-service`, and more); binding port 8080 needs no
capability at all, which is why it binds loopback and leaves 80/443 to the
proxy.

`deploy/cloud-init/user-data.example.yaml` does all of the above on first
boot for a cloud-init VM.

---

## 9. Install path C — Proxmox

Both Proxmox shapes use the systemd unit from §8 unchanged.

- **Unprivileged LXC container** — every hardening directive in the unit is
  LXC-safe, and no nesting is required (nothing runs a container runtime
  inside). This is the lighter option.
- **VM** — use the cloud-init snippet.

Back `/var/lib/anamnesis` with a ZFS dataset or a dedicated mount point so
Proxmox Backup Server captures it.

Both SQLite (in WAL mode) and Postgres are crash-consistent, so a PBS
snapshot of a *running* guest restores cleanly — it is equivalent to a power
cut, which both databases are designed to survive.

---

## 10. Backup and restore

**Back up the blob root in the same operation as the database.** They are one
dataset: an attachment row whose blob is missing is a broken download, and a
blob with no row is invisible. A snapshot that catches one but not the other
produces exactly that.

For SQLite, **never `cp` a live database file.** Use:

```sh
sqlite3 /var/lib/anamnesis/anamnesis.db "VACUUM INTO '/backup/anamnesis.db'"
tar -czf /backup/blobs.tar.gz -C /var/lib/anamnesis blobs
```

For Postgres, `pg_dump` alongside the same blob archive.

Restore is the reverse, with the service stopped: put the database back, put
the blob root back, start the service. Migrations are idempotent, and the
bootstrap admin grant is a no-op when it already exists, so restoring into a
newer version upgrades the schema on first start.

---

## 11. Upgrades

1. Back up (§10).
2. Pull the new image, or replace the binary.
3. Restart.

Migrations run automatically at startup, before the socket binds. There is no
separate migration command and no maintenance window beyond the restart
itself.

Downgrades are not supported: migrations only go forward. Roll back by
restoring the backup from step 1.

---

## 12. Operational constraints

**Anamnesis runs as a single instance.** This is a property of the
application, not of any platform. File attachments are stored on the local
filesystem, and the scheduled-sweep ticker is a per-process singleton that
races other processes on a single row. Two replicas split attachments between
them — half the downloads 404 — and duplicate sweeps, **even against a shared
Postgres**. Do not scale it horizontally. Vertical scaling is fine.

**`/healthz` is both liveness and readiness.** Migrations, bootstrap, and OIDC
discovery all complete *before* the socket binds, so "accepting connections"
genuinely means "ready to serve". The only subtlety is allowing enough startup
time on first boot, when migrations actually have work to do.

**Shutdown is graceful.** `SIGTERM` (what Podman, Docker, Kubernetes and
systemd all send) stops accepting new connections, drains in-flight requests,
and exits — typically in well under a second. A stop that takes the full
timeout means the signal is not reaching the process, usually because it was
wrapped in a shell that does not forward signals.

**No system time zone database is needed.** `ANAMNESIS_TIMEZONE` is validated
at startup against the copy compiled into the binary.

---

## 13. Kubernetes

No manifests are provided — Anamnesis is a single-instance app, which is most
of what a chart would otherwise abstract. These are the constraints a working
manifest has to satisfy:

- **`replicas: 1` with `strategy: Recreate` and an RWO PVC**, for the
  single-instance reason in §12. `RollingUpdate` would briefly run two
  replicas, which is the failure mode above.
- **Probes:** `/healthz` for startup, liveness and readiness. Give the
  `startupProbe` a generous budget — first boot runs migrations.
- **`terminationGracePeriodSeconds`** works as intended; the app handles
  SIGTERM.
- **Secrets:** session secret, OIDC client secret, and database URL in a
  `Secret`. **Pre-create the session secret** — a chart that generates one on
  each upgrade logs out every user, every upgrade (§2).
- **`securityContext`:** `runAsNonRoot: true`, `runAsUser: 65532`,
  `readOnlyRootFilesystem: true`, `capabilities.drop: [ALL]`,
  `seccompProfile: RuntimeDefault`. The image is built for exactly this.
- **Ingress:** must not rewrite paths, and its body limit must be ≥
  `ANAMNESIS_MAX_BODY_BYTES`. On ingress-nginx that is
  `nginx.ingress.kubernetes.io/proxy-body-size`, whose own default is **1 MB**
  and will otherwise be the real ceiling.
- **Postgres** via CNPG or similar; trust the operator's internal CA with
  `?sslrootcert=` on the connection URL (§3), not `ANAMNESIS_TLS_CA_BUNDLE`.
- **Memory limit** sized per §5.
