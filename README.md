# brewx

brewx is a single Rust binary that runs Homebrew executables on demand.
It resolves the executable to a Homebrew formula using an embedded
`db.json`, installs the formula if needed, and then execs the requested
tool.

Example:

```
./target/release/brewx deno --help
```

## Quick start

1. Build the database from cached Homebrew data:

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

`build-db.py` fetches Homebrew API responses (cached in `cache/brew.sh`)
and writes `db.json` in the repository root. The Rust build embeds this
file in the binary, so rebuild after regenerating the DB.

`db.json` contains a schema version, a generation timestamp, and a map of
executable names to their selected Homebrew formula. The builder uses
install analytics to pick the most popular formula when there is more
than one candidate.

## Constraints

- brewx embeds `db.json` at build time; rebuild after regenerating it.
- The database is derived from Homebrew manifests and analytics data.
