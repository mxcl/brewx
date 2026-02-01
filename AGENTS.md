Constructs a database so `pkgx` can execute anything from any package
ecosystem.

Thus we need to store:

* package ecosystem (eg. npm, pip, gem, etc)
* executable name (eg. cowsay, eslint, etc)
* package name (for conflict resolution within the same ecosystem)

The database should be sqlite.

Default to resuming; do not delete the database each time.

Cache files in `./cache`, eg. if we need to download the entire npm registry
cache it in `./cache/npmjs.org`.
