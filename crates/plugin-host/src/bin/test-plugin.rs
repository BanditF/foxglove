//! A Plugin for the runner's tests: `ok` initializes and then waits, silent, until Foxglove
//! closes its stdin; `crash` exits at once with code 3; `probe <file>` exits 7 if it can read
//! the file and 0 if not.

use std::io::{BufRead, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "crash" {
        std::process::exit(3);
    }
    if mode == "probe" {
        // Exit 7 if the file outside the plugin's folder can be read, 0 if not.
        let path = std::env::args().nth(2).unwrap_or_default();
        std::process::exit(if std::fs::read(path).is_ok() { 7 } else { 0 });
    }
    let mut out = std::io::stdout();
    let input = std::io::stdin().lock().lines();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"api":"1.0","client":{"name":"test-plugin","version":"1"}}}"#;
    writeln!(out, "{init}").unwrap();
    out.flush().unwrap();
    // Its reply, then nothing until stdin ends.
    for _ in input {}
}
