# brewx

brewx is a single Rust binary that runs Homebrew executables on demand.
It resolves the executable to a Homebrew formula using an embedded
`db.json`, installs the formula if needed, and then execs the requested
tool.

## Usage

```sh
brewx deno --help
```

## Installation

We’re a single rust binary published to GitHub releases so use [yoink]:

```sh
$ sh <(https://yoink.sh) mxcl/brewx
installed: ~/.local/bin/brewx

# the yoink one-liner installs the latest brewx and that's it
```

Alternatively download from the releases page.


## Caveats

We install everything including deps without symlinks (brew supports this),
thus your `/opt/homebrew/bin` remains what it was before.

Upgrades will still work etc.

[yoink]: https://github.com/mxcl/yoink
