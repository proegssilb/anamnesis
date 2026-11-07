#!/usr/bin/env python3
"""Fail if Cargo.lock gained a crates.io package version younger than N days.

Renovate's `minimumReleaseAge` only governs the requirement Renovate itself
bumps; the versions Cargo.lock resolves transitively -- or that a manual
`cargo update` pulls in -- are not held to it. This closes that gap: every
registry package version that is in Cargo.lock at HEAD but was not at the
base revision must have been published at least `--min-age-days` ago, unless
it is listed in the allow-list file with a reason.

Publish times come from the crates.io sparse index (`pubtime`), falling back
to the crates.io API for old index entries that predate that field.

Standard library only (Python >= 3.11 for tomllib).
"""

import argparse
import json
import subprocess  # nosec B404 -- only runs `git show` with a fixed argv
import sys
import time
import tomllib
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone

CRATES_IO_SOURCES = (
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
)
INDEX_URL = "https://index.crates.io/"
API_URL = "https://crates.io/api/v1/crates/"
USER_AGENT = "anamnesis-check-lock-age (github.com/proegssilb/anamnesis)"
API_MIN_INTERVAL_SECS = 1.0

_last_api_call = [0.0]


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", required=True, help="git revision to compare against")
    parser.add_argument("--head", default="HEAD", help="git revision to check (default HEAD)")
    parser.add_argument("--lockfile", default="Cargo.lock", help="lockfile path in the repo")
    parser.add_argument("--allow", default=".github/lock-age-allow.toml", help="allow-list file")
    parser.add_argument("--min-age-days", type=int, default=14)
    return parser.parse_args(argv)


def git(*args):
    return subprocess.run(  # nosec B603 B607 -- fixed git argv, no shell
        ["git", *args], capture_output=True, text=True, check=False
    )


def lock_at(rev, path):
    """Return Cargo.lock's text at `rev`, or "" if it did not exist there."""
    if git("rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}").returncode != 0:
        raise ValueError(f"unknown git revision {rev!r} (shallow clone?)")
    result = git("show", f"{rev}:{path}")
    return result.stdout if result.returncode == 0 else ""


def registry_packages(lock_text):
    """Set of (name, version) for every crates.io package in a lockfile."""
    packages = tomllib.loads(lock_text).get("package", [])
    return {
        (pkg["name"], pkg["version"])
        for pkg in packages
        if pkg.get("source", "").startswith(CRATES_IO_SOURCES)
    }


def load_allow_list(path):
    """Map (name, version) -> reason. Every entry must carry a reason."""
    try:
        with open(path, "rb") as handle:
            entries = tomllib.load(handle).get("allow", [])
    except FileNotFoundError:
        return {}
    allowed = {}
    for entry in entries:
        reason = str(entry.get("reason", "")).strip()
        if not reason:
            raise ValueError(f"{path}: entry {entry!r} has no reason")
        allowed[(entry["name"], entry["version"])] = reason
    return allowed


def index_path(name):
    """The sparse index's path for a crate name (see the Cargo book)."""
    name = name.lower()
    if len(name) <= 2:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as response:  # nosec B310 -- https constant
        return response.read().decode("utf-8")


def index_pubtimes(name):
    """Map version -> pubtime string (or None) from the sparse index."""
    lines = fetch(INDEX_URL + index_path(name)).splitlines()
    entries = (json.loads(line) for line in lines if line.strip())
    return {entry["vers"]: entry.get("pubtime") for entry in entries}


def api_created_at(name, version):
    """Publish time from the crates.io API, at most one request per second."""
    wait = API_MIN_INTERVAL_SECS - (time.monotonic() - _last_api_call[0])
    if wait > 0:
        time.sleep(wait)
    _last_api_call[0] = time.monotonic()
    body = json.loads(fetch(f"{API_URL}{name}/{version}"))
    return body["version"]["created_at"]


def published_at(name, version, index_cache):
    if name not in index_cache:
        index_cache[name] = index_pubtimes(name)
    stamp = index_cache[name].get(version) or api_created_at(name, version)
    return datetime.fromisoformat(stamp.replace("Z", "+00:00"))


def too_young(new_packages, min_age, now):
    """List (name, version, published, age) for packages younger than min_age."""
    cache = {}
    offenders = []
    for name, version in sorted(new_packages):
        published = published_at(name, version, cache)
        if now - published < min_age:
            offenders.append((name, version, published, now - published))
    return offenders


def report(offenders, allowed, min_age_days):
    """Print the findings; return True if any are not allow-listed."""
    blocking = [o for o in offenders if (o[0], o[1]) not in allowed]
    for name, version, published, age in offenders:
        reason = allowed.get((name, version))
        tag = f"ALLOWED ({reason})" if reason else "TOO NEW"
        print(f"  {tag}: {name} {version} published {published:%Y-%m-%d %H:%M} UTC, "
              f"{age.days}d {age.seconds // 3600}h old")
    if blocking:
        print(f"\n{len(blocking)} package version(s) are younger than {min_age_days} days. "
              "Wait, pin an older version (cargo update -p NAME --precise VER), or add an "
              "entry with a reason to the allow-list.")
    return bool(blocking)


def main(argv=None):
    args = parse_args(argv)
    head = registry_packages(lock_at(args.head, args.lockfile))
    base = registry_packages(lock_at(args.base, args.lockfile))
    new_packages = head - base
    print(f"{len(new_packages)} registry package version(s) new in {args.head} vs {args.base}.")
    if not new_packages:
        return 0
    min_age = timedelta(days=args.min_age_days)
    offenders = too_young(new_packages, min_age, datetime.now(timezone.utc))
    if not offenders:
        print(f"All are at least {args.min_age_days} days old.")
        return 0
    return 1 if report(offenders, load_allow_list(args.allow), args.min_age_days) else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (urllib.error.URLError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(2)
