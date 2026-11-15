# Size budget checker

`scripts/size_budget.py` scans Rust files in `crates/` and `app/src/foxglove/` for functions exceeding 120 lines or files exceeding 1,500 lines. It prints warnings to stdout but always exits with code 0; it never blocks a build gate. The function-length check counts braces from the `fn` keyword to its closing brace after stripping comments and string literals, so it is an estimate rather than a full parse.

## Usage

    python scripts/size_budget.py [--fn-lines 120] [--file-lines 1500]

## Flags

| Flag | Default | Description |
| --- | --- | --- |
| `--fn-lines` | `120` | Maximum allowed lines per function. Functions exceeding this limit are listed as warnings. |
| `--file-lines` | `1500` | Maximum allowed lines per file. Files exceeding this limit are listed as warnings. |

## Exit codes

Always returns 0, regardless of how many violations are found.

## Reads and writes

Reads all `*.rs` files under the repository root in `crates/` and `app/src/foxglove/`, skipping any path containing a `target` component (e.g. generated dependency directories). Writes warning lines to stdout only; no output file is produced.

## Output order

File warnings come first, sorted largest first, in the form `warning: <rel>: N lines (budget N)`. Function warnings follow, sorted longest first, as `warning: <rel>:<line>: fn <name> is N lines (budget N)`. The last line is a summary.

## Dependencies

Python standard library only.

## Example

    python scripts/size_budget.py --fn-lines 100 --file-lines 1500
    warning: crates/foo/big.rs: 1720 lines (budget 1500)
    warning: crates/foo/bar.rs:42: fn process_event is 135 lines (budget 100)
    size budget: 1 files over 1500 lines, 1 functions over 100 lines (warnings only)
