//! The update against a keyboard in memory: no drives, no device, no flashing.

use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

const LEFT_SERIAL: &str = "1E89EF085AA8A635";
const RIGHT_SERIAL: &str = "35B4541F78DE9CF8";

fn image(half: Half, version: &str, profile: &str, sha: &str) -> Vec<u8> {
    let mut payload = vec![0u8; 512];
    payload.extend(
        format!(
            "FOXGLOVE_HALF={};VER={version};PROFILE={profile};SHA={sha}",
            half.name()
        )
        .as_bytes(),
    );
    payload.resize(1024, 0);
    uf2::build(&payload)
}

fn build(version: &str, profile: &str, sha: &str) -> Build {
    Build {
        version: version.into(),
        profile: profile.into(),
        sha: sha.into(),
    }
}

/// The keyboard and the PC in memory. `drives` scripts which bootloaders show after each wait.
struct Mock {
    on_usb: Option<bool>,
    battery: Option<u8>,
    hands_free: bool,
    builds: RefCell<[Option<Build>; 2]>,
    present: RefCell<Vec<String>>,
    /// What appears when a half enters its bootloader (by request or taps).
    arrives: RefCell<VecDeque<Vec<String>>>,
    /// Builds each half runs after a write is taken.
    after: RefCell<[Option<Build>; 2]>,
    /// Writes the bootloader ignores before it takes one.
    ignore: RefCell<u32>,
    gone_mid_write: bool,
    left_back: bool,
    log: RefCell<Vec<String>>,
    written: RefCell<Vec<(String, usize)>>,
    /// Time the waits have taken, on the mock's clock.
    clock: std::cell::Cell<Duration>,
    start: std::time::Instant,
}

impl Default for Mock {
    fn default() -> Self {
        Mock {
            on_usb: Some(true),
            battery: Some(80),
            hands_free: true,
            builds: RefCell::new([
                Some(build("0.1.0", "dev", "aaaaaaa")),
                Some(build("0.1.0", "dev", "aaaaaaa")),
            ]),
            present: RefCell::new(vec![]),
            arrives: RefCell::new(VecDeque::from([
                vec![LEFT_SERIAL.to_string()],
                vec![RIGHT_SERIAL.to_string()],
            ])),
            after: RefCell::new([
                Some(build("0.2.0", "dev", "bbbbbbb")),
                Some(build("0.2.0", "dev", "bbbbbbb")),
            ]),
            ignore: RefCell::new(0),
            gone_mid_write: false,
            left_back: true,
            log: RefCell::new(vec![]),
            written: RefCell::new(vec![]),
            clock: std::cell::Cell::new(Duration::ZERO),
            start: std::time::Instant::now(),
        }
    }
}

impl Mock {
    fn half_of(&self, serial: &str) -> usize {
        usize::from(serial == RIGHT_SERIAL)
    }
}

impl Board for Mock {
    fn left_on_usb(&self) -> Option<bool> {
        self.on_usb
    }
    fn right_battery(&self) -> Option<u8> {
        self.battery
    }
    fn left_serial(&self) -> Option<String> {
        Some(LEFT_SERIAL.into())
    }
    fn build(&self, half: Half) -> Option<Build> {
        self.builds.borrow()[half.index() as usize].clone()
    }
    fn hands_free(&self) -> bool {
        self.hands_free
    }
    fn enter_bootloader(&self, half: Half) -> Result<(), String> {
        self.log.borrow_mut().push(format!("enter {}", half.name()));
        Ok(())
    }
    fn bootloaders(&self) -> Vec<String> {
        self.present.borrow().clone()
    }
    fn wait_bootloaders(&self, timeout: Duration) {
        match self.arrives.borrow_mut().pop_front() {
            Some(next) => *self.present.borrow_mut() = next,
            None => {
                // Nothing arrives: the wait runs out.
                *self.present.borrow_mut() = vec![];
                self.clock.set(self.clock.get() + timeout);
            }
        }
    }
    fn now(&self) -> std::time::Instant {
        self.start + self.clock.get()
    }
    fn volume(&self, serial: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(format!("{serial}:\\")))
    }
    fn board_id(&self, _volume: &Path) -> Result<String, String> {
        Ok("Board-ID: nRF52840-nicenano".into())
    }
    fn settle(&self) {}
    fn write(&self, volume: &Path, data: &[u8]) -> Result<(), WriteError> {
        let serial = volume.to_string_lossy().trim_end_matches(":\\").to_string();
        self.written.borrow_mut().push((serial.clone(), data.len()));
        if *self.ignore.borrow() > 0 {
            *self.ignore.borrow_mut() -= 1;
            return Ok(());
        }
        let i = self.half_of(&serial);
        self.builds.borrow_mut()[i] = self.after.borrow()[i].clone();
        self.present.borrow_mut().clear();
        if self.gone_mid_write {
            return Err(WriteError::DriveGone("device gone".into()));
        }
        Ok(())
    }
    fn wait_drive_gone(&self, _volume: &Path, _timeout: Duration) -> bool {
        self.present.borrow().is_empty()
    }
    fn wait_left_back(&self, serial: &str, _timeout: Duration) -> bool {
        assert_eq!(serial, LEFT_SERIAL, "the left's own serial");
        self.left_back
    }
    fn wait_right_back(&self, _timeout: Duration) -> bool {
        true
    }
    fn record(&self, half: Half, _sha: &str, outcome: &str, _detail: &str) {
        self.log
            .borrow_mut()
            .push(format!("{} {outcome}", half.name()));
    }
}

fn both() -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        Some(image(Half::Left, "0.2.0", "dev", "bbbbbbb")),
        Some(image(Half::Right, "0.2.0", "dev", "bbbbbbb")),
    )
}

fn go(board: &Mock) -> (Result<(), Failure>, Vec<Progress>, Known) {
    let (l, r) = both();
    let p = plan(l, r, board).expect("a plan");
    let mut known = Known::default();
    let mut steps = vec![];
    let result = run(&p, board, &mut known, false, &mut |s| steps.push(s));
    (result, steps, known)
}

#[test]
fn updates_left_then_right_and_verifies_each() {
    let board = Mock::default();
    let (l, r) = both();
    let p = plan(l, r, &board).unwrap();
    assert_eq!(
        p.lines(),
        [
            "Left: 0.1.0 dev aaaaaaa -> 0.2.0 dev bbbbbbb",
            "Right: 0.1.0 dev aaaaaaa -> 0.2.0 dev bbbbbbb"
        ]
    );
    assert!(!p.downgrade && p.hands_free);
    let (result, steps, known) = go(&board);
    assert_eq!(result, Ok(()));
    assert_eq!(
        *board.written.borrow(),
        [
            (LEFT_SERIAL.to_string(), 2048),
            (RIGHT_SERIAL.to_string(), 2048)
        ]
    );
    assert_eq!(
        *board.log.borrow(),
        [
            "enter left",
            "left written",
            "left verified",
            "enter right",
            "right written",
            "right verified"
        ],
        "left first, the right only after the left is verified"
    );
    assert!(steps
        .iter()
        .any(|s| s.half == Half::Left && s.text.contains("Restarting")));
    assert_eq!(
        steps.last().unwrap().text,
        "The right half runs 0.2.0 dev bbbbbbb"
    );
    assert_eq!(
        known,
        Known {
            left: Some(LEFT_SERIAL.into()),
            right: Some(RIGHT_SERIAL.into())
        }
    );
}

#[test]
fn the_wrong_half_never_gets_a_write() {
    // The right's bootloader shows up when the left's is expected.
    let board = Mock::default();
    board
        .arrives
        .borrow_mut()
        .push_front(vec![RIGHT_SERIAL.into()]);
    let (result, _, _) = go(&board);
    let f = result.unwrap_err();
    assert_eq!(f.half, Half::Left);
    assert!(f.message.contains("isn't the left half's"), "{}", f.message);
    assert!(board.written.borrow().is_empty());
    // The left's bootloader during the right's step (it restarted on its own, say).
    let board = Mock::default();
    board.arrives.borrow_mut()[1] = vec![LEFT_SERIAL.into()];
    let (result, _, _) = go(&board);
    assert!(result.unwrap_err().message.contains("is the left half's"));
    assert_eq!(board.written.borrow().len(), 1, "only the left was written");
    // A right half that isn't the one seen before.
    let board = Mock::default();
    let (l, r) = both();
    let p = plan(l, r, &board).unwrap();
    let mut known = Known {
        left: None,
        right: Some("0000000000000000".into()),
    };
    let f = run(&p, &board, &mut known, false, &mut |_| {}).unwrap_err();
    assert!(f.message.contains("seen before"), "{}", f.message);
}

#[test]
fn two_bootloaders_wait_until_one_is_unplugged() {
    let board = Mock::default();
    board
        .arrives
        .borrow_mut()
        .push_front(vec![LEFT_SERIAL.into(), RIGHT_SERIAL.into()]);
    let (result, steps, _) = go(&board);
    assert_eq!(result, Ok(()));
    assert!(steps.iter().any(|s| s.text.starts_with("Two bootloaders")));
    assert_eq!(board.written.borrow()[0].0, LEFT_SERIAL);
}

#[test]
fn a_drive_already_present_stops_everything() {
    let board = Mock::default();
    *board.present.borrow_mut() = vec![RIGHT_SERIAL.into()];
    let (result, _, _) = go(&board);
    assert!(result.unwrap_err().message.contains("already connected"));
    assert!(
        board.written.borrow().is_empty()
            && !board.log.borrow().contains(&"enter left".to_string())
    );
}

#[test]
fn bluetooth_only_or_unknown_connection_and_low_right_battery_are_refused() {
    let (l, r) = both();
    let ble = Mock {
        on_usb: Some(false),
        ..Default::default()
    };
    assert!(plan(l.clone(), r.clone(), &ble)
        .unwrap_err()
        .contains("Bluetooth only"));
    let unknown = Mock {
        on_usb: None,
        ..Default::default()
    };
    assert!(plan(l.clone(), r.clone(), &unknown).is_err());
    let low = Mock {
        battery: Some(9),
        ..Default::default()
    };
    assert!(plan(l.clone(), r, &low)
        .unwrap_err()
        .contains("battery is at 9%"));
    assert!(
        plan(l, None, &low).is_ok(),
        "the left alone doesn't need the right's battery"
    );
}

#[test]
fn bad_images_mixes_and_downgrades_are_caught_before_anything_starts() {
    let board = Mock::default();
    let swapped = plan(
        Some(image(Half::Right, "0.2.0", "dev", "bbbbbbb")),
        None,
        &board,
    )
    .unwrap_err();
    assert!(swapped.contains("for the right half"), "{swapped}");
    let mix = plan(
        Some(image(Half::Left, "0.2.0", "ga", "bbbbbbb")),
        None,
        &board,
    )
    .unwrap_err();
    assert!(mix.contains("left ga and the right dev"), "{mix}");
    let older = plan(
        Some(image(Half::Left, "0.0.9", "dev", "ccccccc")),
        None,
        &board,
    )
    .unwrap();
    assert!(older.downgrade);
    let mut known = Known::default();
    assert!(run(&older, &board, &mut known, false, &mut |_| {})
        .unwrap_err()
        .message
        .contains("downgrade"));
    assert!(board.log.borrow().is_empty(), "nothing touched");
}

#[test]
fn a_half_on_the_same_build_is_skipped() {
    let board = Mock::default();
    board.builds.borrow_mut()[0] = Some(build("0.2.0", "dev", "bbbbbbb"));
    let (l, r) = both();
    let p = plan(l, r, &board).unwrap();
    assert_eq!(p.skipped, [Half::Left]);
    assert_eq!(p.images.len(), 1);
    assert!(p.lines()[0].ends_with("already up to date"));
}

#[test]
fn without_hands_free_it_asks_for_taps() {
    let board = Mock {
        hands_free: false,
        ..Default::default()
    };
    let (result, steps, _) = go(&board);
    assert_eq!(result, Ok(()));
    assert!(steps.iter().any(|s| s
        .text
        .starts_with("Double-tap the reset button on the left half")));
    assert!(!board.log.borrow().iter().any(|l| l.starts_with("enter")));
}

#[test]
fn an_ignored_write_is_tried_once_more_then_fails() {
    let board = Mock::default();
    *board.ignore.borrow_mut() = 1;
    let (result, _, _) = go(&board);
    assert_eq!(result, Ok(()));
    assert_eq!(board.written.borrow().len(), 3, "left twice, right once");
    let board = Mock::default();
    *board.ignore.borrow_mut() = 2;
    let (result, _, _) = go(&board);
    let f = result.unwrap_err();
    assert!(f.message.contains("didn't take"));
    assert!(f.recovery.contains("double-tap its reset button"));
}

#[test]
fn a_vanished_drive_is_judged_by_the_read_back() {
    let board = Mock {
        gone_mid_write: true,
        ..Default::default()
    };
    let (result, _, _) = go(&board);
    assert_eq!(result, Ok(()), "the build read back says it was written");
    assert!(board
        .log
        .borrow()
        .contains(&"left written-drive-gone".to_string()));
    let board = Mock {
        gone_mid_write: true,
        ..Default::default()
    };
    *board.after.borrow_mut() = [Some(build("0.1.0", "dev", "aaaaaaa")), None];
    let (result, _, _) = go(&board);
    assert!(result
        .unwrap_err()
        .message
        .contains("runs 0.1.0 dev aaaaaaa"));
}

#[test]
fn a_left_that_doesnt_come_back_stops_before_the_right() {
    let board = Mock {
        left_back: false,
        ..Default::default()
    };
    let (result, _, _) = go(&board);
    let f = result.unwrap_err();
    assert!(f.message.contains("right wasn't touched"));
    assert_eq!(board.written.borrow().len(), 1);
}

#[test]
fn a_drive_that_never_appears_times_out_with_recovery_steps() {
    let board = Mock {
        hands_free: false,
        ..Default::default()
    };
    board.arrives.borrow_mut().clear();
    let (l, _) = both();
    let p = plan(l, None, &board).unwrap();
    let f = run(&p, &board, &mut Known::default(), false, &mut |_| {}).unwrap_err();
    assert!(f.message.contains("didn't appear"), "{}", f.message);
    assert!(f.recovery.contains("RESCUE.md"));
    assert!(board.written.borrow().is_empty());
    assert!(
        board.clock.get() >= DRIVE_WAIT,
        "it waited the whole window on the board's clock"
    );
}
