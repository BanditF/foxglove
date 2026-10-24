//! How soon each half shows a colour the PC sends (#266). Colours stream to the underglow as the
//! app's lighting sends them (INVOKE `rgb_ug` RGB_COLOR_HSB), each with its own hue, while RGB_STATE
//! is polled at 4 Hz and unsolicited D7 83 reports are collected. A write counts as shown on a half
//! at the first report carrying its hue; one never reported on the right is lost.
use anyhow::{ensure, Result};
use digitalis::{link::Link, protocol, settings};
use serde::Serialize;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

/// RGB_COLOR_HSB in ZMK's `rgb_ug` command list (as the app's lighting sends it).
const RGB_COLOR_HSB: u32 = 14;

#[derive(Clone, Copy, Debug)]
pub struct Write {
    pub at_ms: u64,
    pub hue: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Seen {
    pub at_ms: u64,
    pub left_hue: u16,
    pub right_hue: Option<u16>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Lag {
    pub shown: usize,
    pub median_ms: u64,
    pub p90_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub writes: usize,
    pub writes_refused: usize,
    pub left: Lag,
    pub right: Lag,
    /// Writes the left reported but the right never did: lost on the way to the right. (A write
    /// neither half reported was replaced before any report could show it.)
    pub right_lost: usize,
    /// Right reports going back to an older write after a newer one (newest-wins must never).
    pub right_out_of_order: usize,
    /// Whether the right reported the last colour of the stream (a skipped one in between is
    /// newest-wins working; a missing last one is a real loss).
    pub right_shows_last: bool,
    pub polls: usize,
    /// D7 83 reports beyond the poll replies: unsolicited RGB_STATE events.
    pub events: usize,
}

fn lag(mut ms: Vec<u64>) -> Lag {
    ms.sort_unstable();
    let at = |q: f64| {
        ms.get(((ms.len() as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0)
    };
    Lag {
        shown: ms.len(),
        median_ms: at(0.5),
        p90_ms: at(0.9),
        max_ms: ms.last().copied().unwrap_or(0),
    }
}

/// Matches each write to the first report after it that carries its hue, per half.
pub fn analyse(writes: &[Write], seen: &[Seen]) -> (Lag, Lag, usize) {
    let first = |w: &Write, hue: &dyn Fn(&Seen) -> Option<u16>| {
        seen.iter()
            .find(|s| s.at_ms >= w.at_ms && hue(s) == Some(w.hue))
            .map(|s| s.at_ms - w.at_ms)
    };
    let (mut left, mut right, mut lost) = (Vec::new(), Vec::new(), 0);
    for w in writes {
        let l = first(w, &|s| Some(s.left_hue));
        let r = first(w, &|s| s.right_hue);
        left.extend(l);
        right.extend(r);
        lost += usize::from(l.is_some() && r.is_none());
    }
    (lag(left), lag(right), lost)
}

/// Right reports in write order: (backwards steps, whether the last write was reported).
pub fn order(writes: &[Write], seen: &[Seen]) -> (usize, bool) {
    let index = |hue: u16| writes.iter().position(|w| w.hue == hue);
    let (mut newest, mut back) = (None, 0);
    for i in seen.iter().filter_map(|s| s.right_hue.and_then(index)) {
        if newest.is_some_and(|n| i < n) {
            back += 1;
        }
        newest = newest.max(Some(i));
    }
    (back, !writes.is_empty() && newest == Some(writes.len() - 1))
}

/// Streams `count` colours at `hz`, keeps watching for `settle`, then puts the colour back.
pub fn run(
    link: &Link,
    rgb_ug: u16,
    count: u16,
    hz: f64,
    poll: Duration,
    settle: Duration,
) -> Result<Summary> {
    ensure!(
        (1..=120).contains(&count),
        "writes must be 1..=120 (each needs its own hue)"
    );
    ensure!((0.5..=20.0).contains(&hz), "hz must be 0.5..=20");
    let original = settings::rgb_state(link)?.left;
    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    let _reports = link.subscribe_reports(move |_, r| {
        if r[0] == protocol::SETTINGS && r[1] == 0x80 | protocol::settings::RGB_STATE {
            let _ = tx.send((start.elapsed().as_millis() as u64, *r));
        }
    });
    let step = 360 / (u32::from(count) + 1);
    let hue = |i: u16| ((u32::from(original.hue) + step * (u32::from(i) + 1)) % 360) as u16;
    let (period, poll_every) = (Duration::from_secs_f64(1.0 / hz), poll);
    let (mut writes, mut refused, mut polls) = (Vec::new(), 0, 0);
    let (mut next_write, mut next_poll) = (start, start);
    let end_writes = start + period * u32::from(count);
    while Instant::now() < end_writes + settle {
        let now = Instant::now();
        if writes.len() + refused < usize::from(count) && now >= next_write {
            let h = hue((writes.len() + refused) as u16);
            let hsb = (u32::from(h) << 16)
                | (u32::from(original.sat) << 8)
                | u32::from(original.brightness);
            let at_ms = start.elapsed().as_millis() as u64;
            match settings::invoke(link, rgb_ug, RGB_COLOR_HSB, hsb, 0)? {
                protocol::InvokeStatus::Ok => writes.push(Write { at_ms, hue: h }),
                _ => refused += 1,
            }
            next_write += period;
        }
        if now >= next_poll {
            if settings::rgb_state(link).is_ok() {
                polls += 1;
            }
            next_poll += poll_every;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let restore = (u32::from(original.hue) << 16)
        | (u32::from(original.sat) << 8)
        | u32::from(original.brightness);
    settings::invoke(link, rgb_ug, RGB_COLOR_HSB, restore, 0)?;
    drop(_reports);
    let seen: Vec<Seen> = rx
        .try_iter()
        .map(|(at_ms, r)| {
            let s = protocol::RgbState::parse(&r);
            Seen {
                at_ms,
                left_hue: s.left.hue,
                right_hue: s.right.map(|(h, _)| h.hue),
            }
        })
        .collect();
    let (left, right, right_lost) = analyse(&writes, &seen);
    let (right_out_of_order, right_shows_last) = order(&writes, &seen);
    Ok(Summary {
        writes: writes.len(),
        writes_refused: refused,
        left,
        right,
        right_lost,
        right_out_of_order,
        right_shows_last,
        polls,
        events: seen.len().saturating_sub(polls),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_each_write_to_its_first_report_per_half() {
        let writes = [
            Write { at_ms: 0, hue: 10 },
            Write {
                at_ms: 200,
                hue: 20,
            },
            Write {
                at_ms: 400,
                hue: 30,
            },
        ];
        let seen = [
            Seen {
                at_ms: 5,
                left_hue: 10,
                right_hue: Some(0),
            },
            Seen {
                at_ms: 205,
                left_hue: 20,
                right_hue: Some(10),
            },
            Seen {
                at_ms: 900,
                left_hue: 30,
                right_hue: Some(30),
            },
        ];
        let (left, right, lost) = analyse(&writes, &seen);
        assert_eq!((left.shown, left.median_ms, left.max_ms), (3, 5, 500));
        // Hue 10 reached the right after 205 ms, hue 30 after 500 ms (the upper median); hue 20 never.
        assert_eq!(
            (right.shown, right.median_ms, right.max_ms, lost),
            (2, 500, 500, 1)
        );
    }

    #[test]
    fn order_counts_steps_back_and_the_last_write() {
        let writes = [
            Write { at_ms: 0, hue: 10 },
            Write {
                at_ms: 200,
                hue: 20,
            },
            Write {
                at_ms: 400,
                hue: 30,
            },
        ];
        let at = |hue| Seen {
            at_ms: 0,
            left_hue: 0,
            right_hue: Some(hue),
        };
        assert_eq!(order(&writes, &[at(10), at(30)]), (0, true));
        assert_eq!(order(&writes, &[at(20), at(10), at(20)]), (1, false));
    }

    #[test]
    fn reports_before_a_write_do_not_count() {
        let writes = [Write {
            at_ms: 100,
            hue: 10,
        }];
        let seen = [Seen {
            at_ms: 50,
            left_hue: 10,
            right_hue: Some(10),
        }];
        let (left, right, lost) = analyse(&writes, &seen);
        assert_eq!((left.shown, right.shown, lost), (0, 0, 0));
    }
}
