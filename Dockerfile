# syntax=docker/dockerfile:1

# Anamnesis ships as one binary and one writable directory. Templates, static
# assets, both migration sets, and the IANA time zone database are all
# embedded at compile time, so the runtime image needs no asset sidecar, no
# `sqlx-cli` migration step, and no `tzdata` package.
#
# Nothing here sets DATABASE_URL or SQLX_OFFLINE: this workspace uses runtime
# `sqlx::query` exclusively, never the `query!` macros, so no database is
# reachable — or needed — at build time.

# --- Dependency cache -------------------------------------------------------
#
# `cargo-chef` reduces the workspace to a manifest-only "recipe", so the
# dependency layer is invalidated by Cargo.toml/Cargo.lock changes alone and
# not by every source edit.
FROM rust:1.98-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- Build ------------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin anamnesis-web

# The state directory is created here, in a stage that runs as root, purely so
# the `COPY --chown` below can hand it to the runtime user. See the runtime
# stage for why that matters.
RUN mkdir -p /state/blobs

# --- Runtime ----------------------------------------------------------------
#
# `cc` rather than `static-debian12`: the binary links glibc (via `ring`'s C
# code and the standard library's DNS resolution), so it is not static.
#
# The Debian release MUST match the builder's. `rust:1.98-trixie` is Debian 13
# (glibc 2.41) and produces a binary that will not start on this Debian 12
# base (glibc 2.36); bookworm and `cc-debian12` are the matched pair.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

COPY --from=builder /build/target/release/anamnesis-web /usr/local/bin/anamnesis-web

# Ownership, not just existence, is the point. A fresh named volume is
# initialised from the image directory *including its ownership* — Docker and
# Podman both do this copy-up — so a root-owned /var/lib/anamnesis makes the
# very first start fail with EACCES on both the SQLite file and the blob root,
# as uid 65532 (distroless `nonroot`) cannot write into it.
COPY --from=builder --chown=65532:65532 /state /var/lib/anamnesis

# The `:nonroot` base tag already selects this uid, so this line changes no
# behaviour today -- it removes the dependence on that tag being right. A base
# image bumped to `:latest`, or repinned by digest to the wrong variant, would
# otherwise silently start running as root, and nothing in this file would say
# so. Spelling out the numeric uid also matches the `--chown` above and the
# `runAsUser: 65532` that docs/DEPLOYMENT.md §13 tells Kubernetes operators to
# set; a name would not, since the runtime has no way to resolve one.
USER 65532:65532

ENV ANAMNESIS_BLOB_ROOT=/var/lib/anamnesis/blobs
WORKDIR /var/lib/anamnesis
EXPOSE 8080

# Exec form, and the binary probes itself: this base image has neither a shell
# nor `curl`. `--start-period` allows for migrations, bootstrap, and OIDC
# discovery, all of which complete before the socket is bound.
# Podman builds OCI-format images by default, and the OCI image spec has no
# healthcheck field -- `podman build` prints a warning and DROPS this
# instruction. Build with `--format docker` (as `just image` does) for the
# healthcheck to survive. Docker/BuildKit keeps it either way.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["/usr/local/bin/anamnesis-web", "--health-check"]

ENTRYPOINT ["/usr/local/bin/anamnesis-web"]
