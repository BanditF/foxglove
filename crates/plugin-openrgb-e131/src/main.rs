//! OpenRGB (E1.31), a Foxglove plugin: OpenRGB, or any sACN/E1.31 sender, colours every
//! keyboard's lights. Point an OpenRGB E1.31 device at 127.0.0.1, universe 1, and its colour
//! becomes the keyboards' underglow colour (the average of the LEDs it sends).
//!
//! - Receives on 127.0.0.1:5568 (UDP) only, through Foxglove (`net.listen:udp:127.0.0.1:5568`),
//!   so the plugin itself runs sandboxed, without any network of its own.
//! - Holds each keyboard's lights with a low-priority lease while a stream runs (#73), so
//!   notifications win; the leased colour changes only when the colour does, at most
//!   `--max-hz` times a second.
//! - When the sender stops (stream terminated, or 2.5 s without data, E1.31's loss timeout),
//!   or the plugin stops, the leases end and Foxglove gives the lights back as they were.
//!
//! Options (in `plugin.toml`'s `run`): `--universe N` (1), `--start-channel N` (1), `--leds N`
//! (every LED sent), `--max-hz N` (10). `--send-test R G B [--seq N] [--stop]` sends one frame
//! to the plugin and exits, for testing without OpenRGB.

mod e131;

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_plugin::{Hsb, Lease, LeasePriority, NetEvent, Plugin};

/// E1.31 6.7.1: a stream is lost after 2.5 s without data.
const LOSS_TIMEOUT: Duration = Duration::from_millis(2500);

struct Options {
    universe: u16,
    start_channel: usize,
    leds: Option<usize>,
    max_hz: u32,
    /// The UDP port (5568, E1.31's); tests use a free one of their own.
    port: u16,
}

fn options(args: &[String]) -> Options {
    let num = |flag: &str| -> Option<u64> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .and_then(|w| w[1].parse().ok())
    };
    Options {
        universe: num("--universe").map_or(1, |n| n as u16),
        start_channel: num("--start-channel").map_or(1, |n| (n as usize).max(1)),
        leds: num("--leds").map(|n| n as usize),
        max_hz: num("--max-hz").map_or(10, |n| (n as u32).clamp(1, 30)),
        port: num("--port")
            .and_then(|n| u16::try_from(n).ok())
            .filter(|p| *p != 0)
            .unwrap_or(e131::PORT),
    }
}

/// The average colour of the RGB triples from `start_channel` (1-based), `leds` of them or
/// all there are; `None` with no whole LED.
fn average(channels: &[u8], start_channel: usize, leds: Option<usize>) -> Option<(u8, u8, u8)> {
    let rgb = channels.get(start_channel - 1..)?;
    let n = leds.map_or(rgb.len() / 3, |l| l.min(rgb.len() / 3));
    if n == 0 {
        return None;
    }
    let sum = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .take(n)
        .fold([0u32; 3], |s, c| {
            [
                s[0] + u32::from(c[0]),
                s[1] + u32::from(c[1]),
                s[2] + u32::from(c[2]),
            ]
        });
    let avg = |v: u32| (v / n as u32) as u8;
    Some((avg(sum[0]), avg(sum[1]), avg(sum[2])))
}

/// RGB to the device layer's HSB (hue 0-359, saturation and brightness 0-100).
fn hsb((r, g, b): (u8, u8, u8)) -> Hsb {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    Hsb {
        hue: (hue.round() as u16) % 360,
        saturation: (saturation * 100.0).round() as u8,
        brightness: (max * 100.0).round() as u8,
    }
}

/// The keyboards with lights, each held with a low-priority lease in `color` (#73): a
/// notification (mic muted, a focus timer) outranks the sync, and when the stream ends, or the
/// plugin stops or crashes, Foxglove gives the lights back as they were.
fn take_over(plugin: &Arc<Plugin>, color: Hsb) -> Vec<Lease> {
    plugin
        .devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.connected && d.caps.iter().any(|c| c == "lighting"))
        .filter_map(|d| {
            plugin
                .lease(&d.id, color, LeasePriority::Low)
                .map_err(|e| eprintln!("openrgb-e131: lighting: {e}"))
                .ok()
        })
        .collect()
}

fn send_test(args: &[String], universe: u16, port: u16) -> std::io::Result<()> {
    let at = args.iter().position(|a| a == "--send-test").unwrap_or(0);
    let v: Vec<u8> = args[at + 1..]
        .iter()
        .take(3)
        .filter_map(|a| a.parse().ok())
        .collect();
    let [r, g, b] = v[..] else {
        eprintln!("openrgb-e131: --send-test R G B");
        std::process::exit(2);
    };
    // --seq N numbers the frame (a repeat of the last number counts as stale); --stop marks it
    // the stream's end.
    let seq = args
        .windows(2)
        .find(|w| w[0] == "--seq")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(1);
    let options = if args.iter().any(|a| a == "--stop") {
        0x40
    } else {
        0
    };
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let frame: Vec<u8> = [r, g, b].repeat(8);
    // Each test sender is its own source, as separate senders are in E1.31.
    let mut cid = *b"foxglove-test-00";
    let unique = std::process::id()
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
    cid[12..].copy_from_slice(&unique.to_be_bytes());
    socket.send_to(
        &e131::build(cid, universe, seq, options, &frame),
        (Ipv4Addr::LOCALHOST, port),
    )?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = options(&args);
    if args.iter().any(|a| a == "--send-test") {
        if let Err(e) = send_test(&args, opts.universe, opts.port) {
            eprintln!("openrgb-e131: {e}");
            std::process::exit(1);
        }
        return;
    }
    let plugin = match Plugin::connect("openrgb-e131", env!("CARGO_PKG_VERSION")) {
        Ok(plugin) => plugin,
        Err(e) => {
            eprintln!("openrgb-e131: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    // Localhost only: other machines can't drive the lights. Foxglove does the listening.
    let frames = match plugin.listen_udp(opts.port) {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("openrgb-e131: can't listen on 127.0.0.1:{}: {e}", opts.port);
            std::process::exit(1);
        }
    };
    let period = Duration::from_millis(1000 / u64::from(opts.max_hz));
    // The last sequence number per sender (E1.31 numbers frames per source).
    let mut last_seq: Option<([u8; 16], u8)> = None;
    // Held while a stream runs; dropping them gives the lights back.
    let mut leases: Option<Vec<Lease>> = None;
    let mut last_data = Instant::now();
    let mut wanted: Option<Hsb> = None;
    let mut sent: Option<Hsb> = None;
    let mut next_send = Instant::now();
    loop {
        // Foxglove gone (stopped, or killed): don't linger.
        if plugin.is_closed() {
            return;
        }
        let mut stop = false;
        // Idle: block until a frame comes (#281). Showing: wake also when the stream counts as
        // lost, or when a held-back colour may go out (one-shot deadlines).
        let deadline = leases.is_some().then(|| {
            let lost = last_data + LOSS_TIMEOUT;
            if wanted.is_some() && wanted != sent {
                lost.min(next_send)
            } else {
                lost
            }
        });
        let event = match deadline {
            Some(at) => frames.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => frames
                .recv()
                .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
        };
        let received = match event {
            Ok(NetEvent::Data(bytes, from)) => Some((bytes, from.unwrap_or_default())),
            // Foxglove stopped listening, or went away.
            Ok(NetEvent::Closed(why)) => {
                eprintln!("openrgb-e131: Foxglove stopped listening: {why}");
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if let Some((buf, from)) = received {
            if let Some(p) = e131::parse(&buf).filter(|p| p.universe == opts.universe && !p.preview)
            {
                if last_seq
                    .is_some_and(|(cid, last)| cid == p.cid && e131::is_stale(last, p.sequence))
                    && !p.terminated
                {
                    continue;
                }
                last_seq = Some((p.cid, p.sequence));
                last_data = Instant::now();
                if p.terminated {
                    stop = true;
                } else if let Some(rgb) = average(p.channels, opts.start_channel, opts.leds) {
                    let color = hsb(rgb);
                    if leases.is_none() {
                        eprintln!("openrgb-e131: first frame from {from} (universe {}): taking the lights", p.universe);
                        leases = Some(take_over(&plugin, color));
                        sent = Some(color);
                        next_send = Instant::now() + period;
                    }
                    wanted = Some(color);
                }
            }
        }
        let now = Instant::now();
        if leases.is_some() && (stop || now.duration_since(last_data) > LOSS_TIMEOUT) {
            // The leases end; Foxglove puts the lights back (and confirms them).
            (leases, wanted, sent, last_seq) = (None, None, None, None);
            continue;
        }
        if let (Some(held), Some(color)) = (&leases, wanted) {
            if sent != Some(color) && now >= next_send {
                for lease in held {
                    if let Err(e) = lease.set(color) {
                        eprintln!("openrgb-e131: lighting: {e}");
                    }
                }
                sent = Some(color);
                next_send = now + period;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_the_chosen_leds() {
        let ch = [255, 0, 0, 0, 0, 255, 9, 9, 9];
        assert_eq!(average(&ch, 1, Some(2)), Some((127, 0, 127)));
        assert_eq!(average(&ch, 4, None), Some((4, 4, 132)));
        assert_eq!(average(&ch, 8, None), None);
        assert_eq!(average(&ch, 20, None), None);
    }

    #[test]
    fn rgb_to_hsb() {
        assert_eq!(
            hsb((255, 0, 0)),
            Hsb {
                hue: 0,
                saturation: 100,
                brightness: 100
            }
        );
        assert_eq!(
            hsb((0, 255, 0)),
            Hsb {
                hue: 120,
                saturation: 100,
                brightness: 100
            }
        );
        assert_eq!(
            hsb((0, 0, 128)),
            Hsb {
                hue: 240,
                saturation: 100,
                brightness: 50
            }
        );
        assert_eq!(
            hsb((255, 0, 255)),
            Hsb {
                hue: 300,
                saturation: 100,
                brightness: 100
            }
        );
        assert_eq!(
            hsb((0, 0, 0)),
            Hsb {
                hue: 0,
                saturation: 0,
                brightness: 0
            }
        );
        assert_eq!(hsb((255, 255, 255)).saturation, 0);
    }

    #[test]
    fn options_parse_and_clamp() {
        let a: Vec<String> = ["--universe", "4", "--start-channel", "0", "--max-hz", "99"]
            .map(String::from)
            .to_vec();
        let o = options(&a);
        assert_eq!(
            (o.universe, o.start_channel, o.leds, o.max_hz),
            (4, 1, None, 30)
        );
    }
}
