# local_config.py: this PC's values for the dev tooling

The dev tooling used to bake in this PC's keyboard serials and paths; `scripts/local_config.py` is the one place they now live, and the other scripts ask here. Each value comes from the first of these that has it: an environment variable (listed below), then the first `tools/local.toml` found, then the built-in defaults, which are this PC's values, so nothing changes here without a file. The `identity` values are the one exception: off by default, on only when a `local.toml` or the environment turns them on.

## How to run

    python scripts/local_config.py get worktrees.root
    python scripts/local_config.py show

There are no flags: `main` accepts only the two forms above. `get` takes one argument, a dotted key of the form `<section>.<key>`; `show` takes none.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | `get` printed the value, or `show` printed all values |
| 2 | Any other invocation; the module docstring (usage) is printed to stderr |

`get` looks the key up directly, so an unknown key raises a `KeyError` instead of using an exit code.

## What it reads and writes

**Reads:** the environment variables `FOXGLOVE_LEFT_SERIAL`, `FOXGLOVE_RIGHT_SERIAL`, `FOXGLOVE_ROLLBACK`, `FOXGLOVE_WORKSPACE`, `FOXGLOVE_WORKTREES`, `FOXGLOVE_SPARSE_IDENTITY`, `FOXGLOVE_IDENTITY_PUBLISHER`; then the first existing `tools/local.toml` of: the path named by `FOXGLOVE_LOCAL_TOML`, the checkout's own `tools/local.toml`, then the workspace's `tools/local.toml` (the workspace is the folder holding the main checkout, found by running `git -C <repo> rev-parse --path-format=absolute --git-common-dir`, asked only when the earlier places have no file); then the built-in defaults: `keyboard.left_serial`, `keyboard.right_serial`, `flash.rollback`, `workspace.root`, `worktrees.root`, `identity.sparse`, `identity.publisher`.

**Writes:** nothing on disk. `get` prints the one value to stdout; `show` prints one `section.key = value` line per value to stdout; on a bad invocation the usage text goes to stderr.

An environment value for `identity.sparse` is on only when it is exactly `1`; any other value (for example `0`) is off.

## As a module

Importers can also call `serials()`, which returns `{left_serial: 'left', right_serial: 'right'}` and refuses serials that are not plain ASCII letters and digits, or that are the same for both halves.

## Example

    python scripts/local_config.py get worktrees.root
    C:\Users\fuchs\splitkeeb\worktrees

The built-in default for `worktrees.root` is `C:\Users\fuchs\splitkeeb\worktrees`, so when no environment variable and no `tools/local.toml` override it, `get` prints that.
