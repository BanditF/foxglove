#!/usr/bin/env python3
"""Source lint: no test may open a real window, take focus, or start a visible console.

Why: a test binary that creates a window while the user is in a game steals focus (2026-11-20).
Tests run in this repo's gate, so a window-creating test must be `#[ignore = "creates a real
window; ..."]` (run on purpose with FOXGLOVE_QUIET_DESKTOP=1 and --ignored), or be listed with a
reason in scripts/window_tests_allowlist.txt after someone read it and saw it is window-free.

What counts as test code: a `#[cfg(test)] mod ...` to the end of its file, and every file under a
`tests/` folder or named `*_tests.rs` / `tests.rs`. app/vendor, vendor (upstream copies, outside every workspace, never run) and target
folders are skipped.

A hit in a test region passes when its enclosing `fn` has `#[ignore` among its attributes, or the
allowlist names `path::fn` or `path` (whole file). For `Command::new` the file may instead use
CREATE_NO_WINDOW / creation_flags (a console child with no window).

Exit 1 with the list of unvetted hits. `--self-test` checks the scanner on made-up text.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = Path(__file__).resolve().parent / "window_tests_allowlist.txt"

WINDOW = re.compile(
    r"\b(CreateWindowExW?|CreateWindowW?|ShowWindow(Async)?|SetForegroundWindow|SetFocus|"
    r"AllowSetForegroundWindow|SwitchToThisWindow|BringWindowToTop|FlashWindowEx|SendInput|"
    r"keybd_event|mouse_event|Shell_NotifyIcon|TrayIconBuilder|NotifyIcon|MessageBoxW?|"
    r"ShellExecuteExW?|ShellExecuteW?|ToastNotification|ToastNotificationManager|"
    r"run_native|run_simple_native|run_app_on_demand|pump_events|RegisterClassW?|"
    r"SetWindowPos|rfd|open::that)\b|EventLoop(Builder)?::|WindowBuilder|create_window\(|"
    r"ViewportBuilder::|\.run_app\("
)
SPAWN = re.compile(r"\bCommand::new\(|\bCommand::new\b")
NO_WINDOW = re.compile(r"CREATE_NO_WINDOW|creation_flags|0x0800_0000|FOXGLOVE_QUIET_DESKTOP")
FN = re.compile(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?fn\s+(\w+)")


def test_regions(path: str, lines):
    """(first, last_exclusive) line index pairs of test code."""
    if re.search(r"(^|/)tests?/|_tests\.rs$|(^|/)tests\.rs$", path):
        return [(0, len(lines))]
    out = []
    i = 0
    while i < len(lines):
        if (
            lines[i].strip() == "#[cfg(test)]"
            and i + 1 < len(lines)
            and re.match(r"\s*(pub(\([a-z]+\))? )?mod \w+", lines[i + 1])
            and not lines[i + 1].rstrip().endswith(";")  # `mod x;` is its own file
        ):
            depth, seen, j = 0, False, i + 1
            while j < len(lines):
                c = code_of(lines[j])
                depth += c.count("{") - c.count("}")
                seen = seen or "{" in c
                if seen and depth <= 0:
                    break
                j += 1
            out.append((i, min(j + 1, len(lines))))
            i = j
        i += 1
    return out


def allowed_set():
    out = set()
    if ALLOWLIST.exists():
        for raw in ALLOWLIST.read_text(encoding="utf-8").splitlines():
            line = raw.split("#", 1)[0].strip()
            if line:
                out.add(line)
    return out


def code_of(line: str) -> str:
    s = line.strip()
    if s.startswith("//"):
        return ""
    s = re.sub(r'"(?:[^"\\]|\\.)*"', '""', s)  # string literals: a name in text is not a call
    return s.split("//", 1)[0]


def scan(path: str, text: str, allow=frozenset()):
    lines = text.split("\n")
    regions = test_regions(path, lines)
    if not regions:
        return []
    file_spawn_ok = bool(NO_WINDOW.search(text))
    hits = []
    for i in (k for a, z in regions for k in range(a, z)):
        code = code_of(lines[i])
        if not code:
            continue
        kind = None
        if WINDOW.search(code):
            kind = "window"
        elif SPAWN.search(code) and not file_spawn_ok:
            kind = "spawn"
        if not kind:
            continue
        # enclosing fn: nearest fn line above (at or before i) at lower-or-equal indent
        name = None
        fn_line = None
        for j in range(i, -1, -1):
            m = FN.match(lines[j])
            if m:
                name, fn_line = m.group(1), j
                # a nested helper fn keeps looking up when it is not a test itself
                attrs = " ".join(lines[max(0, j - 8):j])
                if "#[test]" in attrs or "#[ignore" in attrs or "#[tokio::test" in attrs:
                    break
                indent = len(lines[j]) - len(lines[j].lstrip())
                if indent <= 4:
                    break
        attrs = " ".join(lines[max(0, (fn_line or i) - 8):(fn_line or i)])
        if "#[ignore" in attrs:
            continue
        if path in allow or f"{path}::{name}" in allow:
            continue
        hits.append((i + 1, name or "?", kind, lines[i].strip()[:90]))
    return hits


def tracked_rs():
    out = subprocess.check_output(["git", "ls-files", "*.rs"], cwd=ROOT, text=True)
    for p in out.splitlines():
        if p.startswith(("app/vendor/", "vendor/")) or "/target/" in p:
            continue
        yield p


def self_test():
    ok = "#[cfg(test)]\nmod t {\n    #[test]\n    #[ignore = \"creates a real window\"]\n    fn a() { CreateWindowExW(); }\n}\n"
    bad = "#[cfg(test)]\nmod t {\n    #[test]\n    fn a() { ShowWindow(h); }\n}\n"
    spawn_bad = "#[cfg(test)]\nmod t {\n    #[test]\n    fn a() { Command::new(\"cmd\"); }\n}\n"
    spawn_ok = spawn_bad + "// CREATE_NO_WINDOW\n"
    prod = "fn main() { CreateWindowExW(); }\n"
    text_only = "#[cfg(test)]\nmod t {\n    #[test]\n    fn a() { let _ = \"SendInput\"; }\n}\n"
    assert not scan("a.rs", ok)
    assert len(scan("a.rs", bad)) == 1
    assert not scan("a.rs", bad, {"a.rs::a"}) and not scan("a.rs", bad, {"a.rs"})
    assert len(scan("a.rs", spawn_bad)) == 1 and not scan("a.rs", spawn_ok)
    assert not scan("a.rs", prod)
    assert not scan("a.rs", text_only)
    assert len(scan("x/tests/b.rs", "fn a() { SetForegroundWindow(h); }\n")) == 1
    print("self-test ok")


def main():
    if "--self-test" in sys.argv:
        return self_test()
    allow = allowed_set()
    bad = []
    for p in tracked_rs():
        try:
            text = (ROOT / p).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for ln, fn, kind, src in scan(p, text, allow):
            bad.append(f"{p}:{ln} in fn {fn} ({kind}): {src}")
    if bad:
        print("Tests that may create a window, take focus or start a visible console, not ignored or vetted:")
        for b in bad:
            print("  " + b)
        print(
            '\nFix: #[ignore = "creates a real window; set FOXGLOVE_QUIET_DESKTOP=1 and run with --ignored on a quiet desktop"],\n'
            "child processes with .creation_flags(CREATE_NO_WINDOW), or (after reading it: no window, no focus)\n"
            "a `path::fn` line with a reason in scripts/window_tests_allowlist.txt."
        )
        return 1
    print("window-test lint: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
