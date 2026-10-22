//! Prints every report the keyboard sends that isn't a reply: layer state (0xFF), key events
//! (0xF1) and anything else. Run it, then hold a layer key:
//!   cargo run -p digitalis --example tap [product filter] [seconds]

use std::time::{Duration, Instant};

use digitalis::hid::Match;
use digitalis::link::Link;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let product = args.next().filter(|p| !p.is_empty());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    let link = Link::hid(Match { product, without_host_link: true, ..Match::default() })?;
    let c = link.wait_connected(Duration::from_secs(10))?;
    let host_link = match c.hello {
        Some(h) => format!("host link v{}, panel {}x{}", h.version, h.width, h.height),
        None => "no host link".into(),
    };
    println!("connected: {} ({host_link}); listening for {seconds} s", c.product);

    let reports = link.subscribe(|_| true);
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        let Ok(r) = reports.recv_timeout(Duration::from_millis(200)) else { continue };
        match r[0] {
            0xFF => {
                let size = (r[1] as usize).clamp(1, 4);
                let mask = |at: usize| r[at..at + size].iter().rev().fold(0u32, |m, b| m << 8 | *b as u32);
                println!("layers: default {:#06b}, active {:#06b}", mask(2), mask(2 + size));
            }
            0xF1 => println!("key r{} c{} {}", r[1], r[2], if r[3] != 0 { "down" } else { "up" }),
            _ => println!("other: {:02x?}", &r[..8]),
        }
    }
    Ok(())
}
