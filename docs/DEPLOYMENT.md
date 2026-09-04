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

The state directory holds file attachments (unless `ANAMNESIS_BLOB_ROOT`
points at an object store — §12) and, if you use SQLite, the database file — *two* files, in fact: the data one you
configure, and a `-leases` sibling the app derives from it (§3). Both live
under `/var/lib/anamnesis` in every example here.

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
| `ANAMNESIS_BLOB_ROOT` | `./blobs` | A directory path, or an `s3://bucket/prefix` URL — see §12 |
| `ANAMNESIS_S3_ACCESS_KEY_ID` | unset | **Required** when `ANAMNESIS_BLOB_ROOT` is an `s3://` URL |
| `ANAMNESIS_S3_SECRET_ACCESS_KEY` | unset | **Required** when `ANAMNESIS_BLOB_ROOT` is an `s3://` URL |
| `ANAMNESIS_S3_ENDPOINT` | AWS's own | e.g. `https://garage.example.com:3900` — always set it for Garage or MinIO |
| `ANAMNESIS_S3_REGION` | `us-east-1` | Must match what the server was configured with; it is signed over |
| `ANAMNESIS_MAX_BODY_BYTES` | `41943040` (40 MiB) | Whole-request limit — see §5 |
| `ANAMNESIS_TLS_CA_BUNDLE` | unset | PEM bundle of extra roots for the IdP — see §6 |
| `ANAMNESIS_OIDC_SCOPES` | `openid profile email` | |
| `RUST_LOG` | `info` | |

The four `ANAMNESIS_S3_*` variables are read **only** when
`ANAMNESIS_BLOB_ROOT` starts with `s3://`; on a filesystem root they are
ignored rather than rejected, so moving a deployment back to local disk does
not mean unsetting them all. When the root *is* an `s3://` URL, the two
credential variables are required and a missing one fails startup by name,
like any other required variable.

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

**SQLite uses a second file for coordination.** Alongside the database you
configure, the app opens a sibling with `-leases` before the extension —
`/var/lib/anamnesis/anamnesis-leases.db` for the URL above — carrying the same
query string, and creates it on first boot. It holds only the `job_leases`
table (§12): who is currently migrating or sweeping, and until when.

It is a separate *file* rather than a table because SQLite's write lock covers
a whole database. A lease living in the data file could not be renewed while a
migration held that file's lock, which is exactly when renewing matters.
Postgres has no such problem — its row locks are per row — so there the leases
are simply another table in the one database.

Two operational consequences:

- **Mount the directory, not the file.** A bind mount of a single database
  file leaves the app unable to create its sibling next to it. Mount
  `/var/lib/anamnesis` and let both files live inside — which is what §7's and
  §8's examples already do.
- **It is disposable, but not while anything is running.** The file holds no
  user data and never needs backing up or restoring; a fresh one is created on
  next boot. Deleting it out from under live instances, though, deletes the
  agreement they are relying on, and two of them can then migrate or sweep at
  once. Stop the service first.

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

The database runs in WAL mode, so the state directory also holds `-wal` and
`-shm` files next to it. That is another reason `cp` is wrong — it captures a
database whose most recent commits are still sitting in a file it did not copy.
`VACUUM INTO` writes one consistent file that needs neither sidecar.

The `-leases` sibling (§3) is deliberately absent from that command. It holds
coordination state, not data — who is sweeping right now — which is meaningless
by the time you restore. A restore recreates it empty.

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

**Anamnesis can run as several instances sharing one machine.** The request
path holds no server-side state, and the background work that used to be a
per-process singleton is now coordinated through the database: startup
bootstrap, the scheduled sweep, and tangle detection are each taken under a
lease in a `job_leases` table, so exactly one instance runs each. What the
others do when they lose depends on the job: the sweep moves on, bootstrap and
tangle detection wait their turn (see below, and "Migrations serialize through
the same lease").

**Tangle detection runs on the instance that changed the graph, under a
lease.** Detecting knots in the blocking graph, and closing out placed tangles
that are no longer cyclic, both used to run inline on every board request —
a system-wide reconciliation write on the busiest read path, and, across
instances, N concurrent passes racing each other's inserts. They are now
driven by the only two events that can change their answer: creating and
deleting a `blocks` edge. The instance serving that request runs the pass
itself, before responding, so the board a user is redirected to already
reflects the change. No viewer pays for a pass, and the board stays
immediately consistent.

The lease makes those passes single-writer. An instance that cannot get it
*waits* for it (up to 5s) rather than skipping, which is what stops a
concurrent edit from being lost: a pass that starts after its own commit
cannot fail to see it. So a `blocks` edit can queue briefly behind another
one; nothing else can.

A backstop ticker runs the same pass every 15 minutes, and that one skips
rather than waits. It exists only to repair what the event path missed — a
process killed between the commit and the pass, a lease left behind by a
crashed instance, a pass that failed against a database that has since
recovered. A pass recomputes its whole answer from the graph, so it is a
complete repair and leaves nothing partial behind. Both intervals are
compile-time constants (`BACKSTOP_INTERVAL`, `LEASE_WAIT`), not configuration
knobs.

**Migrations serialize through the same lease.** On Postgres they would anyway
— sqlx holds a per-database advisory lock across the whole run — but on SQLite
sqlx takes no lock at all, and the busy timeout does not stand in for one: its
migrator reads which migrations have been applied *outside* any transaction and
then applies them inside one, so two instances can both read "none applied" and
both try. The loser fails with `SQLITE_ERROR` (`table areas already exists`),
not `SQLITE_BUSY`, and no timeout retries that. So an instance takes the
`migrations` lease before migrating, renewing it every 10s against a 30s
expiry, and any instance that cannot get it waits — with no deadline, since a
migration takes as long as it takes — logging every 30s while it does.

That wait is the one startup case that can visibly exceed a health check's
patience: on an upgrade with real schema work, the second instance's socket
does not bind until the first has finished. Size `startupProbe` budgets for the
migration, not for the process (§13).

| Topology | Coordinates through | |
|---|---|---|
| SQLite, one process | nothing to coordinate | Supported |
| SQLite, N processes, **one machine** | the database file | Supported |
| Postgres, N processes, **one machine** | the database | Supported |
| Postgres, N processes, **N machines** | the database and an object store | Supported — see below |
| SQLite, N machines | — | No, and not planned |

**Attachments decide whether instances can span machines.**
`ANAMNESIS_BLOB_ROOT` picks the backend from its own scheme, the same way
`ANAMNESIS_DATABASE_URL` picks a database driver:

| `ANAMNESIS_BLOB_ROOT` | Backend | Instances may span machines |
|---|---|---|
| `/var/lib/anamnesis/blobs` | Local filesystem | No |
| `s3://bucket/prefix` | S3-compatible object store | Yes |

A **filesystem** root is right for one machine, whether that is one process or
several — they share the directory and are fine. Instances on *separate*
machines each see only the blobs they themselves wrote, so roughly half of all
downloads 404. Writes are atomic (written to a temporary name in the same
directory, then renamed), so an instance killed mid-upload leaves no truncated
blob for another instance to serve as whole; it can leave a stray `.tmp-…`
file, which is inert.

An **object store** is what lifts that restriction. It is the last piece of
shared state that was not already in the database, so with Postgres and an
`s3://` root there is nothing left pinning instances to one host. Point it at
whatever you run — this was written against [Garage](https://garagehq.deuxfleurs.fr/),
and MinIO and S3 itself work the same way:

```sh
ANAMNESIS_BLOB_ROOT=s3://anamnesis/blobs
ANAMNESIS_S3_ENDPOINT=https://garage.example.com:3900
ANAMNESIS_S3_REGION=garage
ANAMNESIS_S3_ACCESS_KEY_ID=GK…
ANAMNESIS_S3_SECRET_ACCESS_KEY=…
```

Four things worth knowing before you switch:

- **The bucket must already exist.** Anamnesis never creates it, and nothing
  is contacted at startup — S3 has no connection to open, so a wrong endpoint,
  a wrong credential or a missing bucket first surfaces on an attachment
  upload, not on boot.
- **Nothing migrates existing attachments.** Blobs already on disk stay on
  disk; copy the blob root's contents under the URL's prefix before switching,
  or those downloads 404.
- **Requests are path-style** (`{endpoint}/{bucket}/{key}`), which is what
  self-hosted endpoints expect and what AWS still accepts. A virtual-hosted
  endpoint is not configurable.
- **Memory is unchanged.** Attachments are still read and written whole, so
  §5's *limit × concurrent uploads* ceiling applies exactly as before — an
  object store buys shared storage, not streaming.

**Every instance needs an identical `ANAMNESIS_SESSION_SECRET`.** Sessions are
signed cookies with no server-side state, so nothing needs sharing and no
sticky sessions are required — but a cookie signed by one instance has to
verify at the next, and a differing secret logs a user out on whichever
requests land elsewhere (§2).

**Distributed SQLite is not supported and is not a planned direction.** What
SQLite instances coordinate *through* is the file; N machines with N files
have nothing in common to coordinate through. Use Postgres.

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

No manifests are provided. These are the constraints a working manifest has to
satisfy:

- **Replicas depend on the blob root, not on coordination.** §12's leases make
  concurrent instances safe either way. With a filesystem blob root on an RWO
  PVC, the volume binds to a single node and a second replica cannot mount it,
  so that manifest needs `replicas: 1` with `strategy: Recreate` —
  `RollingUpdate` stalls waiting for a volume rather than corrupting anything,
  but it is not a zero-downtime upgrade either. With Postgres and an `s3://`
  blob root there is no PVC and no such limit: run `replicas: N` with
  `RollingUpdate`, and size the `startupProbe` for a migration wait (§12), not
  just for process start.
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
