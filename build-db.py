#!/usr/bin/env python3
import datetime
import hashlib
import json
import os
import base64
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed

FORMULA_URL = "https://formulae.brew.sh/api/formula.json"
ANALYTICS_URL = "https://formulae.brew.sh/api/analytics/install/30d.json"
CACHE_DIR = "cache"
ECOSYSTEM = "brew.sh"
DB_PATH = "db.json"
SCHEMA_VERSION = 1
META_KEY = "__pkgdb_meta__"
PAYLOAD_KEY = "__pkgdb_payload__"
USER_AGENT = "brewx/0.1"
CHECK_INTERVAL_SECONDS = 24 * 60 * 60
DEFAULT_TIMEOUT = 60
MANIFEST_ACCEPT = "application/vnd.oci.image.index.v1+json"
TOKEN_SERVICE = "https://ghcr.io/token"

_GHCR_TOKENS = {}


def _ensure_cwd():
    root = os.path.abspath(os.path.dirname(__file__))
    cwd = os.path.abspath(os.getcwd())
    if cwd != root:
        print(
            "build-db.py must be run from the repository root.",
            file=sys.stderr,
        )
        sys.exit(2)


def _cache_path(url):
    digest = hashlib.sha256(url.encode("utf-8")).hexdigest()
    return os.path.join(CACHE_DIR, ECOSYSTEM, f"{digest}.json")


def _read_cached_json(url):
    path = _cache_path(url)
    if not os.path.exists(path):
        raise FileNotFoundError(path)
    with open(path, "rb") as handle:
        data = json.load(handle)
    if isinstance(data, dict) and META_KEY in data and PAYLOAD_KEY in data:
        meta = data.get(META_KEY) or {}
        return data.get(PAYLOAD_KEY), meta
    return data, {}


def _write_cache(path, payload, etag, checked_at):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    wrapper = {
        META_KEY: {"etag": etag, "checked_at": checked_at},
        PAYLOAD_KEY: payload,
    }
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(wrapper, handle)


def _fetch_json(url):
    path = _cache_path(url)
    payload = None
    meta = {}
    if os.path.exists(path):
        payload, meta = _read_cached_json(url)

    checked_at = meta.get("checked_at")
    now = int(time.time())
    if (
        isinstance(checked_at, int)
        and now - checked_at < CHECK_INTERVAL_SECONDS
    ):
        return payload

    headers = {"Accept": "application/json", "User-Agent": USER_AGENT}
    parsed = urllib.parse.urlparse(url)
    if parsed.hostname == "ghcr.io":
        headers["Accept"] = MANIFEST_ACCEPT
        repo = _ghcr_repo_from_url(parsed.path)
        if repo:
            token = _ghcr_bearer_token(repo)
            if token:
                headers["Authorization"] = f"Bearer {token}"
    etag = meta.get("etag")
    if etag:
        headers["If-None-Match"] = etag

    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(
            request,
            timeout=DEFAULT_TIMEOUT,
        ) as response:
            data = response.read()
            etag = response.headers.get("etag")
            payload = json.loads(data)
            _write_cache(path, payload, etag, now)
            return payload
    except urllib.error.HTTPError as err:
        if err.code == 404:
            return None
        if err.code == 304 and payload is not None:
            _write_cache(path, payload, etag, now)
            return payload
        if payload is not None:
            print(f"Using cached data for {url}: {err}", file=sys.stderr)
            return payload
        raise
    except urllib.error.URLError as err:
        if payload is not None:
            print(f"Using cached data for {url}: {err}", file=sys.stderr)
            return payload
        raise


def _github_token():
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if token:
        return token.strip()
    try:
        result = subprocess.run(
            ["gh", "auth", "token"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None
    token = result.stdout.strip()
    if token:
        return token
    return None


def _github_username():
    for key in ("GHCR_USERNAME", "GITHUB_ACTOR", "USER"):
        value = os.environ.get(key)
        if value:
            return value.strip()
    try:
        result = subprocess.run(
            ["gh", "api", "user", "-q", ".login"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None
    username = result.stdout.strip()
    if username:
        return username
    return None


def _ghcr_repo_from_url(path):
    parts = [part for part in path.split("/") if part]
    if len(parts) < 4 or parts[0] != "v2":
        return None
    return "/".join(parts[1:-2])


def _ghcr_bearer_token(repo):
    now = int(time.time())
    cached = _GHCR_TOKENS.get(repo)
    if cached and cached["expires_at"] > now:
        return cached["token"]

    token = _github_token()
    if not token:
        return None
    username = _github_username() or "x-access-token"
    scope = f"repository:{repo}:pull"
    query = urllib.parse.urlencode(
        {"service": "ghcr.io", "scope": scope}
    )
    url = f"{TOKEN_SERVICE}?{query}"
    basic = base64.b64encode(
        f"{username}:{token}".encode("utf-8")
    ).decode("utf-8")
    headers = {
        "Authorization": f"Basic {basic}",
        "User-Agent": USER_AGENT,
    }
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(
            request,
            timeout=DEFAULT_TIMEOUT,
        ) as response:
            data = json.loads(response.read())
    except urllib.error.HTTPError as err:
        print(f"Failed to get GHCR token for {repo}: {err}", file=sys.stderr)
        return None
    bearer = data.get("token")
    expires_in = data.get("expires_in", 300)
    if bearer:
        _GHCR_TOKENS[repo] = {
            "token": bearer,
            "expires_at": now + int(expires_in) - 10,
        }
        return bearer
    return None


def _stable_version(stable):
    if isinstance(stable, str):
        return stable
    if isinstance(stable, dict):
        for key in ("version", "tag"):
            value = stable.get(key)
            if value:
                return value
    return None


def _manifest_url(formula):
    name = formula.get("name")
    versions = formula.get("versions", {})
    stable = versions.get("stable")
    version = _stable_version(stable)
    if not name or not version:
        return None

    url = (
        "https://ghcr.io/v2/homebrew/core/"
        f"{name.replace('+', 'x')}/manifests/{version}"
    )

    revision = formula.get("revision")
    stable_revision = None
    if isinstance(stable, dict):
        stable_revision = stable.get("revision")
    revision_value = revision if revision is not None else stable_revision
    if revision_value not in (None, 0):
        url = f"{url}_{revision_value}"

    rebuild = (
        formula.get("bottle", {})
        .get("stable", {})
        .get("rebuild")
    )
    if rebuild:
        url = f"{url}-{rebuild}"

    return url


def _parse_count(value):
    if value is None:
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        value = value.replace(",", "")
        if value.isdigit():
            return int(value)
    return None


def _fetch_popularity():
    payload = _fetch_json(ANALYTICS_URL)
    items = payload.get("items") if isinstance(payload, dict) else None
    popularity = {}
    if not isinstance(items, list):
        return popularity
    for item in items:
        if not isinstance(item, dict):
            continue
        formula = item.get("formula")
        count = _parse_count(item.get("count"))
        if formula and count is not None:
            popularity[formula] = count
    return popularity


def _parse_exec_paths(paths):
    executables = set()
    for entry in paths:
        if not entry:
            continue
        entry = entry.strip()
        if not entry:
            continue
        name = entry.rsplit("/", 1)[-1]
        if name:
            executables.add(name)
    return executables


def _collect_entries(formulae, popularity_by_formula, manifests):
    entries = {}
    missing_manifests = 0
    for formula in formulae:
        if not isinstance(formula, dict):
            continue
        name = formula.get("name")
        if not name or "@" in name:
            continue
        popularity = popularity_by_formula.get(name, 0)

        url = _manifest_url(formula)
        if not url:
            continue

        payload = manifests.get(url)
        if not payload:
            missing_manifests += 1
            continue

        manifest_list = payload.get("manifests", [])
        executables = set()
        for manifest in manifest_list:
            annotations = None
            if isinstance(manifest, dict):
                annotations = manifest.get("annotations")
            if not annotations:
                continue
            provides = annotations.get("sh.brew.path_exec_files")
            if not provides:
                continue
            paths = [
                item.strip()
                for item in provides.split(",")
                if item.strip()
            ]
            executables.update(_parse_exec_paths(paths))
            if executables:
                break

        for executable in executables:
            entries.setdefault(executable, []).append(
                {"formula": name, "popularity": popularity}
            )

    return entries, missing_manifests


def _sorted_entries(entries):
    ordered = {}
    for executable in sorted(entries.keys()):
        items = entries[executable]
        items.sort(
            key=lambda item: (
                -(item.get("popularity") or 0),
                item.get("formula", ""),
            )
        )
        ordered[executable] = items
    return ordered


def main():
    _ensure_cwd()

    os.makedirs(os.path.join(CACHE_DIR, ECOSYSTEM), exist_ok=True)

    formulae = _fetch_json(FORMULA_URL)
    if not isinstance(formulae, list):
        print("Formula list was not a list.", file=sys.stderr)
        sys.exit(2)

    popularity_by_formula = {}
    try:
        popularity_by_formula = _fetch_popularity()
    except Exception as err:
        print(f"Failed to fetch analytics data: {err}", file=sys.stderr)

    manifest_urls = []
    for formula in formulae:
        if not isinstance(formula, dict):
            continue
        name = formula.get("name")
        if not name or "@" in name:
            continue
        url = _manifest_url(formula)
        if url:
            manifest_urls.append(url)

    manifests = {}
    completed = 0
    max_workers = min(32, (os.cpu_count() or 4) * 4)
    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        future_map = {
            executor.submit(_fetch_json, url): url for url in manifest_urls
        }
        for future in as_completed(future_map):
            url = future_map[future]
            try:
                payload = future.result()
            except Exception as err:
                print(f"Failed to fetch {url}: {err}", file=sys.stderr)
                continue
            if payload:
                manifests[url] = payload
            completed += 1
            if completed % 20 == 0:
                print(
                    f"Fetched {completed}/{len(manifest_urls)} manifests...",
                    file=sys.stderr,
                )

    entries, missing_manifests = _collect_entries(
        formulae,
        popularity_by_formula,
        manifests,
    )
    ordered_entries = _sorted_entries(entries)

    db = {
        "schema": SCHEMA_VERSION,
        "generated_at": datetime.datetime.now(
            datetime.timezone.utc
        ).isoformat(),
        "entries": ordered_entries,
    }

    with open(DB_PATH, "w", encoding="utf-8") as handle:
        json.dump(db, handle, indent=2, sort_keys=True)
        handle.write("\n")

    print(f"Wrote {DB_PATH} with {len(ordered_entries)} executables")
    if missing_manifests:
        print(
            f"Skipped {missing_manifests} formulas missing cached manifests",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main()
