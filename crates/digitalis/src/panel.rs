//! The left screen's host panel: a 1-bit image, rows MSB first, 1 = ink, as the firmware takes it.

use anyhow::Result;
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};

use std::time::Duration;

use crate::protocol::{self, Hello};
use crate::Transport;

pub struct Panel {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bits: Vec<u8>,
}

impl Panel {
    pub fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(8);
        Panel { width, height, stride, bits: vec![0; stride * height] }
    }

    pub fn clear(&mut self) {
        self.bits.fill(0);
    }

    pub fn set(&mut self, x: i32, y: i32, on: bool) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let i = y * self.stride + x / 8;
        let bit = 0x80 >> (x % 8);
        if on {
            self.bits[i] |= bit;
        } else {
            self.bits[i] &= !bit;
        }
    }
}

impl OriginDimensions for Panel {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl DrawTarget for Panel {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            self.set(p.x, p.y, c.is_on());
        }
        Ok(())
    }
}

/// Sends the rows of `frame` that differ from `last` (all of them without one), then PRESENT.
/// Returns how many reports went out.
pub fn show(t: &impl Transport, frame: &[u8], last: Option<&[u8]>, stride: usize) -> Result<usize> {
    let rows = frame.len() / stride;
    let changed = |y: usize| match last {
        Some(l) => frame[y * stride..(y + 1) * stride] != l[y * stride..(y + 1) * stride],
        None => true,
    };
    let per_report = protocol::panel::rows_per_report(stride);
    let mut sent = 0;
    let mut y = 0;
    while y < rows {
        if !changed(y) {
            y += 1;
            continue;
        }
        let n = per_report.min(rows - y);
        t.send(&protocol::rows(y, &frame[y * stride..(y + n) * stride], stride))?;
        sent += 1;
        y += n;
    }
    t.send(&protocol::present())?;
    Ok(sent + 1)
}

/// Asks the keyboard for its panel size, right-half link and battery.
pub fn hello(t: &impl Transport) -> Result<Hello> {
    let r = t.request(&protocol::hello(), protocol::PANEL, protocol::panel::HELLO, Duration::from_millis(500))?;
    Ok(Hello::parse(&r))
}

/// Every screen the keyboard describes (D5 06, HELLO capability `SCREENS`, #160): index 0
/// first for the count, then the rest.
pub fn screens(t: &impl Transport) -> Result<Vec<protocol::ScreenDescriptor>> {
    let ask = |i: u8| -> Result<protocol::ScreenDescriptor> {
        let r = t.request(&protocol::screens(i), protocol::PANEL, protocol::panel::SCREENS, Duration::from_millis(500))?;
        protocol::ScreenDescriptor::parse(&r).map_err(|status| anyhow::anyhow!("screen {i}: status {status}"))
    };
    let first = ask(0)?;
    let mut all = vec![first.clone()];
    for i in 1..first.count {
        all.push(ask(i)?);
    }
    Ok(all)
}

/// The PC-drawable panel among `screens` (the first panel that takes rows).
pub fn drawable_panel(screens: &[protocol::ScreenDescriptor]) -> Option<&protocol::ScreenDescriptor> {
    screens
        .iter()
        .find(|s| s.kind == protocol::ScreenKind::Panel && s.ops & protocol::ScreenDescriptor::OP_ROWS != 0 && s.format == 0)
}

/// Gives the area back to the firmware's own drawing.
pub fn release(t: &impl Transport) -> Result<()> {
    t.send(&protocol::release())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Records what was sent; answers requests from a queue of canned replies.
    #[derive(Default)]
    pub struct Mock {
        pub sent: RefCell<Vec<Vec<u8>>>,
        pub replies: RefCell<VecDeque<[u8; protocol::REPORT]>>,
    }

    impl Transport for Mock {
        fn send(&self, report: &[u8]) -> Result<()> {
            self.sent.borrow_mut().push(report.to_vec());
            Ok(())
        }

        fn request(&self, report: &[u8], proto: u8, cmd: u8, _: Duration) -> Result<[u8; protocol::REPORT]> {
            self.send(report)?;
            let r = self.replies.borrow_mut().pop_front().ok_or_else(|| anyhow::anyhow!("no reply"))?;
            anyhow::ensure!(protocol::is_reply(&r, proto, cmd), "wrong reply");
            Ok(r)
        }
    }

    fn screen(count: u8, index: u8, id: u8, kind: u8, w: u16, h: u16, ops: u16) -> [u8; protocol::REPORT] {
        let mut r = [0u8; protocol::REPORT];
        r[..5].copy_from_slice(&[protocol::PANEL, 0x80 | protocol::panel::SCREENS, 0, count, index]);
        r[5] = id;
        r[6] = kind;
        r[8] = if kind == 0 { 0xff } else { 0 };
        r[9..11].copy_from_slice(&w.to_le_bytes());
        r[11..13].copy_from_slice(&h.to_le_bytes());
        r[19..21].copy_from_slice(&ops.to_le_bytes());
        r
    }

    #[test]
    fn reads_every_screen_and_finds_the_panel() {
        let t = Mock::default();
        t.replies.borrow_mut().extend([
            screen(3, 0, 0, 0, 68, 160, 2),
            screen(3, 1, 1, 1, 64, 52, 1),
            screen(3, 2, 3, 2, 68, 140, 12),
        ]);
        let all = screens(&t).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(t.sent.borrow().iter().map(|r| r[2]).collect::<Vec<_>>(), vec![0, 1, 2]);
        let panel = drawable_panel(&all).unwrap();
        assert_eq!((panel.id, panel.width, panel.height), (1, 64, 52));
        assert!(drawable_panel(&all[..1]).is_none());
    }

    #[test]
    fn sends_only_changed_rows() {
        let t = Mock::default();
        let mut p = Panel::new(64, 52);
        assert_eq!(show(&t, &p.bits, None, p.stride).unwrap(), 19); // 18 ROWS + PRESENT
        let first = p.bits.clone();

        t.sent.borrow_mut().clear();
        p.set(0, 10, true); // row 10
        p.set(63, 11, true); // row 11
        p.set(5, 51, true); // last row
        show(&t, &p.bits, Some(&first), p.stride).unwrap();
        let sent = t.sent.borrow();
        assert_eq!(sent.len(), 3);
        assert_eq!(&sent[0][..4], &[0xD5, 0x03, 10, 3]); // rows 10-12 in one report
        assert_eq!(sent[0][4], 0x80);
        assert_eq!(sent[0][4 + 8 + 7], 0x01);
        assert_eq!(&sent[1][..4], &[0xD5, 0x03, 51, 1]);
        assert_eq!(sent[1][4], 0x04);
        assert_eq!(sent[2], protocol::present());
    }
}
