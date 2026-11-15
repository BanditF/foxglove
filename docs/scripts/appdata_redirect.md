# appdata_redirect.py

Redirected AppData (#894), the Python twin of crates/redirect (standard library only). A script started from a packaged app's shell (a Claude Code session, say) runs inside that app's file-system container. Windows then puts new files under `%APPDATA%` and `%LOCALAPPDATA%` into that app's private copy (`%LOCALAPPDATA%\Packages\<package>\LocalCache\...`), which nothing outside the container reads. The flasher's `flash-history.log` and the lab's `lab-injections.log` landed there. This module detects that case after a write and names the package responsible. It takes no command-line arguments; tools import it.

## Functions

| Name | Signature | Description |
| --- | --- | --- |
| `final_path` | `final_path(path)` | Returns the path Windows really opened for the existing file `path` (opens a handle and calls `GetFinalPathNameByHandleW` through ctypes), or `None` when not on Windows, the file does not exist, or Windows cannot say. |
| `redirected` | `redirected(requested, real)` | Given the requested path and the path Windows opened, returns `(package, real_plain)` when `real` lies under another app's package `LocalCache`. Returns `None` for the same file (any case, with or without `\?\`), a link elsewhere, or Foxglove's own package. |
| `check` | `check(path, final=final_path)` | Calls `final(path)` then `redirected(path, real)`. Returns `(package, real_plain)` or `None`. |
| `note` | `note(what, requested, found)` | Returns the one-line message a tool prints when `what`, asked for at `requested`, went to the copy `found`. Does not print. |
| `unc_path` | `unc_path(path)` | Converts `C:\Users\u\...` into `\localhost\C$\Users\u\...`, an administrative share that the container does not redirect (#911). Returns `None` for a path without a drive letter. |

## Own package

`OWN_PACKAGE = 'BanditF.Foxglove'`. A package whose name starts with `BanditF.Foxglove_` is treated as Foxglove's own and is never reported as "another app's" copy. The redirect happens when a script is run from a packaged app's shell, because that shell runs inside the app's container; the fix is to run from a normal terminal (Windows Terminal, Start menu).

## Class

`Notice(what, final=final_path)` checks a file once after a write. `after_write(path, out)` takes the written path and an output stream with a `write` method (for example `sys.stderr`). On its first call it checks the file; if the file was redirected, it prints the `note` line to `out` and returns `(package, real_plain)`, which is also kept in `self.found`. Later calls do nothing and return `None`.

## Platform

Only `final_path` is Windows-gated (it returns `None` when `os.name != 'nt'`). `redirected`, `note` and `unc_path` are pure string functions and run anywhere.

## Example

```python
import sys
from scripts.appdata_redirect import Notice

notice = Notice("flash-history.log")
# ... the tool writes flash-history.log under LOCALAPPDATA ...
found = notice.after_write(r"C:\Users\u\AppData\Local\Foxglove\flash-history.log", out=sys.stderr)
if found:
    package, real_path = found  # note line already printed to stderr
```
