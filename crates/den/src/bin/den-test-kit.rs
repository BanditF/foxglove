//! A Kit for the runner's tests: `ok` initializes and pings every 200 ms, `crash` exits at
//! once with code 3, `hang` initializes and then goes silent.

use std::io::{BufRead, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "crash" {
        std::process::exit(3);
    }
    let mut out = std::io::stdout();
    let mut input = std::io::stdin().lock().lines();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"api":"1.0","client":{"name":"test-kit","version":"1"}}}"#;
    writeln!(out, "{init}").unwrap();
    out.flush().unwrap();
    let _ = input.next();
    let mut id = 1;
    loop {
        if mode == "hang" {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
        id += 1;
        if writeln!(out, r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#).and_then(|()| out.flush()).is_err() {
            return;
        }
        if input.next().is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
