# brewx

brewx is a single Rust binary that runs Homebrew executables on demand.
It resolves the executable to a Homebrew formula using a local `db.json`
file, installs the formula if needed, and then execs the requested tool.

Example:

```
./target/release/brewx deno --help
```

## Quick start

1. Build the database from the existing cached Homebrew data:

```
./build-db.py
```

2. Build the binary:

```
cargo build --release
```

3. Run a Homebrew tool:

```
./target/release/brewx deno --help
```

## Database

`build-db.py` reads cached Homebrew API responses from `cache/brew.sh` and
writes `db.json` in the repository root. It never performs network
requests, so the cache must already exist.

`db.json` contains a schema version, a generation timestamp, and a map of
executable names to ordered formula candidates. brewx picks the first
formula for each executable, so entries are sorted by popularity and then
name.

## Constraints

- brewx expects `db.json` in the current working directory.
- The database is derived from Homebrew manifests and analytics data that
  are already cached in this repository.
