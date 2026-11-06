use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

/// A healthy left as the firmware sends it after a while: relay found in time, nothing counted.
fn left() -> Block {
    Block {
        version: 1,
        half: 0,
        flags: flags::RELAY_CENTRAL
            | flags::HID_STATS
            | flags::HOST_LINK
            | flags::LIGHTING
            | flags::PC_STATE
            | flags::POLARITY
            | flags::DISPLAY,
        firmware: protocol::FirmwareVersion {
            major: 0,
            minor: 1,
            patch: 0,
            profile: 0,
        },
        sha: "c068bcc".into(),
        uptime_s: 3725,
        reset_cause: 1 << 3,
        self_test: [1, 1, 1, 1],
        relay_debug: [3, 1, 0, 0, 0, 40, 0, 2, 23, 0],
        relay_ready: 1,
        relay_discovery: 4,
        hid_sent: 1200,
        split_up_at_s: 18,
        split_connected: true,
        pc_state: 1,
        pc_reason: 0,
        polarity: 0,
        polarity_right: 0,
        leds: 21,
        leds_right: 21,
        art_set: 0,
        ..Block::default()
    }
}

fn right() -> Block {
    Block {
        half: 1,
        flags: flags::RELAY_PERIPHERAL | flags::LIGHTING | flags::POLARITY | flags::DISPLAY,
        pc_reason: 0xff,
        polarity_right: 0xff,
        leds_right: 0,
        hid_sent: 0,
        relay_debug: [0; 10],
        ..left()
    }
}

fn report(l: Block, r: Block) -> Report {
    Report {
        left: Ok(l),
        right: Ok(r),
    }
}

fn find<'a>(rows: &'a [HealthRow], item: &str) -> Vec<&'a HealthRow> {
    rows.iter().filter(|r| r.item == item).collect()
}

#[test]
fn a_block_round_trips_through_its_bytes() {
    let b = Block {
        relay_timeouts: 3,
        art_failed: 2,
        art_last_error: 6,
        art_set: -1,
        relay_discovery: -12,
        redraws: 70_000,
        ..left()
    };
    let bytes = b.to_bytes();
    assert_eq!(bytes.len(), SIZE);
    assert_eq!(Block::parse(&bytes).unwrap(), b);
}

#[test]
fn offsets_are_the_firmwares() {
    // docs/health.md's table, byte by byte, as widgets/health.h's struct lays them out.
    let mut b = vec![0u8; SIZE];
    b[0] = 1;
    b[1] = 1;
    b[2] = 96;
    b[3] = 0x08;
    b[4..8].copy_from_slice(&[0, 1, 2, 1]);
    b[8..12].copy_from_slice(b"abcd");
    b[16..20].copy_from_slice(&7200u32.to_le_bytes());
    b[24] = 2 | (1 << 2); // display failed, code 1
    b[26] = 2 | (2 << 2); // relay failed, code 2
    b[38] = 5; // relay timeouts
    b[42] = 2; // relay found twice
    b[46] = 9; // refused
    b[54] = 4; // Raw HID dropped
    b[58] = 3; // reply drops
    b[60..64].copy_from_slice(&123_456u32.to_le_bytes());
    b[66] = 2; // art failed
    b[68] = 4; // bad blob
    b[69] = 0xff; // set -1
    b[70] = 1; // snapshot errors
    b[82] = 6; // split drops
    b[84..88].copy_from_slice(&60u32.to_le_bytes());
    b[88] = 1;
    b[90] = 0xff;
    b[94] = 7;
    let p = Block::parse(&b).unwrap();
    assert_eq!(
        (p.version, p.half, p.flags),
        (1, 1, flags::RELAY_PERIPHERAL)
    );
    assert_eq!(p.firmware.to_string(), "0.1.2");
    assert_eq!(p.sha, "abcd");
    assert_eq!(p.uptime_s, 7200);
    assert_eq!(
        p.check(0),
        Check {
            state: CheckState::Fail,
            code: 1
        }
    );
    assert_eq!(p.check(1).state, CheckState::Pending);
    assert_eq!(
        p.check(2),
        Check {
            state: CheckState::Fail,
            code: 2
        }
    );
    assert_eq!(
        (p.relay_timeouts, p.relay_ready, p.relay_refused),
        (5, 2, 9)
    );
    assert_eq!((p.hid_dropped, p.hid_reply_drops), (4, 3));
    assert_eq!(p.redraws, 123_456);
    assert_eq!((p.art_failed, p.art_last_error, p.art_set), (2, 4, -1));
    assert_eq!(p.snap_errors, 1);
    assert_eq!(
        (p.split_drops, p.split_up_at_s, p.split_connected),
        (6, 60, true)
    );
    assert_eq!((p.pc_state, p.leds), (0xff, 7));
}

#[test]
fn short_or_empty_blocks_are_refused() {
    assert!(Block::parse(&[1; 40]).is_err());
    assert!(Block::parse(&[0; SIZE]).unwrap_err().contains("version 0"));
    let mut b = left().to_bytes();
    b[2] = 50;
    assert!(Block::parse(&b).is_err());
    // A later version with more bytes: the known fields still read.
    let mut longer = left().to_bytes();
    longer[0] = 2;
    longer[2] = 120;
    longer.resize(120, 0xee);
    assert_eq!(Block::parse(&longer).unwrap().sha, "c068bcc");
}

#[test]
fn thresholds_are_data() {
    let t = Thresholds::default();
    assert_eq!(t.level("relay_timeouts", 0), Level::Ok);
    assert_eq!(t.level("relay_timeouts", 1), Level::Warn);
    assert_eq!(t.level("relay_timeouts", 10), Level::Fail);
    assert_eq!(t.level("split_drops", 2), Level::Ok);
    assert_eq!(t.level("not_a_counter", 999), Level::Ok);
    let t = Thresholds::parse("# a comment\nart_failed = 0, 2 # never a warning\n\n").unwrap();
    assert_eq!(t.level("art_failed", 1), Level::Ok);
    assert_eq!(t.level("art_failed", 2), Level::Fail);
    assert!(Thresholds::parse("art_failed = x, 2").is_err());
    assert!(Thresholds::parse("art_failed 1 2")
        .unwrap_err()
        .contains("line 1"));
}

#[test]
fn every_shipped_threshold_names_a_counter_the_rows_use() {
    let t = Thresholds::default();
    let names: Vec<&str> = t.entries.iter().map(|(n, _, _)| n.as_str()).collect();
    for n in [
        "relay_timeouts",
        "relay_unsticks",
        "relay_refused",
        "hid_reply_drops",
        "hid_dropped",
        "hid_failed",
        "art_failed",
        "snap_errors",
        "light_sync_fails",
        "rgb_mismatches",
        "split_drops",
    ] {
        assert!(names.contains(&n), "{n} missing from data/health.toml");
    }
    assert_eq!(
        names.len(),
        11,
        "a counter in data/health.toml that no row reads"
    );
}

#[test]
fn a_healthy_keyboard_is_all_ok_with_one_line_per_feature() {
    let rows = super::rows(&report(left(), right()), &Thresholds::default());
    assert_eq!(worst(&rows), Level::Ok, "{rows:#?}");
    for item in [
        "Left half",
        "Right half",
        "Left screen",
        "Right screen",
        "Relay to the right half",
        "Reports to the PC",
        "Link between the halves",
        "Stay awake",
    ] {
        assert!(!find(&rows, item).is_empty(), "no {item} row: {rows:#?}");
    }
    let left_half = &find(&rows, "Left half")[0].detail;
    assert!(left_half.contains("0.1.0 (dev) c068bcc"), "{left_half}");
    assert!(left_half.contains("up 1 h 2 min") && left_half.contains("power on"));
    assert!(summary(&rows).starts_with("keyboard health OK"));
    assert!(failed_lines(&rows).is_empty());
}

#[test]
fn failed_art_pushes_read_as_the_issue_asks() {
    let l = Block {
        art_failed: 2,
        art_last_error: 6,
        ..left()
    };
    let rows = super::rows(&report(l, right()), &Thresholds::default());
    let screen: Vec<_> = find(&rows, "Right screen")
        .into_iter()
        .filter(|r| r.subsystem == "screens")
        .collect();
    assert_eq!(screen.len(), 1, "{rows:#?}");
    assert_eq!(
        screen[0].line(),
        "Right screen: 2 art pushes failed (relay timeout or write failure)"
    );
    assert_eq!(screen[0].level, Level::Warn);
    // At the shipped failure level it is failed, and the log's failed set has it.
    let l = Block {
        art_failed: 5,
        art_last_error: 7,
        ..left()
    };
    let rows = super::rows(&report(l, right()), &Thresholds::default());
    assert_eq!(
        failed_lines(&rows),
        vec!["Right screen: 5 art pushes failed (the right refused the art)".to_string()]
    );
    // The right's own count names its state.
    let r = Block {
        art_failed: 1,
        art_last_error: 3,
        ..right()
    };
    let rows = super::rows(&report(left(), r), &Thresholds::default());
    assert!(rows
        .iter()
        .any(|x| x.line() == "Right screen: 1 art push failed (bad CRC)"));
}

#[test]
fn the_boot_self_test_says_what_failed() {
    let l = Block {
        self_test: [2 | (1 << 2), 0, 2 | (1 << 2), 3],
        ..left()
    };
    let rows = super::rows(&report(l, right()), &Thresholds::default());
    let screen = find(&rows, "Left screen");
    assert!(screen
        .iter()
        .any(|r| r.level == Level::Fail && r.detail.contains("device not ready")));
    let settings = &find(&rows, "Left saved settings")[0];
    assert_eq!(
        (settings.level, settings.detail.as_str()),
        (Level::Warn, "still starting up")
    );
    let relay = find(&rows, "Relay to the right half");
    assert!(relay
        .iter()
        .any(|r| r.level == Level::Warn && r.detail.contains("late")));
    assert!(find(&rows, "Left lights").is_empty(), "n/a has no row");
    let l = Block {
        self_test: [1, 1, 2 | (2 << 2), 1],
        ..left()
    };
    let rows = super::rows(&report(l, right()), &Thresholds::default());
    assert!(failed_lines(&rows)
        .contains(&"Relay to the right half: no link to the other half after start".to_string()));
}

#[test]
fn the_relay_raw_hid_and_lights_counters_have_rows() {
    let l = Block {
        relay_timeouts: 2,
        relay_unsticks: 1,
        relay_in_flight: 2,
        hid_dropped: 25,
        hid_reply_drops: 1,
        light_sync_fails: 4,
        rgb_mismatches: 1,
        split_drops: 3,
        ..left()
    };
    let rows = super::rows(&report(l, right()), &Thresholds::default());
    let lines: Vec<String> = rows.iter().map(HealthRow::line).collect();
    for want in [
        "Relay to the right half: stuck: both request slots are held by unanswered requests",
        "Relay to the right half: 2 requests timed out",
        "Relay to the right half: the link was dropped 1 time to free a stuck request",
        "Reports to the PC: 1 reply dropped (the reply queue was full)",
        "Reports to the PC: 25 reports dropped (the USB queue was full, or USB reset)",
        "Right lights: 4 updates to the right half failed",
        "Right lights: showed another colour than the left sent 1 time",
        "Link between the halves: Left half saw it drop 3 times",
    ] {
        assert!(
            lines.iter().any(|l| l == want),
            "missing {want:?} in {lines:#?}"
        );
    }
    assert_eq!(worst(&rows), Level::Fail);
    let s = summary(&rows);
    assert!(
        s.starts_with("keyboard health failed: ") && s.contains("[warning]"),
        "{s}"
    );
}

#[test]
fn the_halves_are_compared() {
    let r = Block {
        sha: "1234567".into(),
        leds: 7,
        ..right()
    };
    let l = Block {
        polarity: 1,
        polarity_right: 0xff,
        split_connected: false,
        ..left()
    };
    let rows = super::rows(&report(l, r), &Thresholds::default());
    let lines: Vec<String> = rows.iter().map(HealthRow::line).collect();
    assert!(lines
        .iter()
        .any(|l| l.starts_with("Both halves: different firmware")));
    assert!(lines
        .iter()
        .any(|l| l == "Right lights: the left drives 21 LEDs on the right, the right has 7"));
    assert!(lines
        .iter()
        .any(|l| l == "Right screen: hasn't confirmed the light theme yet"));
    assert!(lines
        .iter()
        .any(|l| l == "Link between the halves: the right half isn't connected"));
}

#[test]
fn read_errors_become_rows() {
    let r = Report {
        left: Err(ReadError::NotBuilt),
        right: Err(ReadError::NotBuilt),
    };
    let rows = super::rows(&r, &Thresholds::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].level, Level::Warn);
    let r = Report {
        left: Ok(left()),
        right: Err(ReadError::OldFirmware),
    };
    let rows = super::rows(&r, &Thresholds::default());
    assert!(rows.iter().any(|x| x.item == "Right half"
        && x.level == Level::Warn
        && x.detail.contains("flash both")));
    let r = Report {
        left: Ok(left()),
        right: Err(ReadError::NoRelay),
    };
    let rows = super::rows(&r, &Thresholds::default());
    assert!(failed_lines(&rows).contains(
        &"Right half: couldn't read its health: no relay link to the right half".to_string()
    ));
}

#[test]
fn reset_and_uptime_words() {
    assert_eq!(reset_words(0), "not known");
    assert_eq!(reset_words(1 << 3), "power on");
    assert_eq!(reset_words((1 << 3) | (1 << 4)), "the watchdog");
    assert_eq!(reset_words(1 << 20), "0x100000");
    assert_eq!(duration_words(45), "45 s");
    assert_eq!(duration_words(600), "10 min");
    assert_eq!(duration_words(90_061), "1 d 1 h");
}

/// Replies queued per request, as the keyboard would send them.
#[derive(Default)]
struct Fake {
    asked: RefCell<Vec<Vec<u8>>>,
    replies: RefCell<VecDeque<[u8; REPORT]>>,
}

impl Transport for Fake {
    fn send(&self, _: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("unexpected send")
    }
    fn request(
        &self,
        report: &[u8],
        proto: u8,
        cmd: u8,
        timeout: Duration,
    ) -> anyhow::Result<[u8; REPORT]> {
        assert_eq!(
            (proto, cmd),
            (protocol::SETTINGS, protocol::settings::HEALTH)
        );
        assert!(timeout <= Duration::from_secs(8));
        self.asked.borrow_mut().push(report.to_vec());
        self.replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("no reply from the keyboard"))
    }
}

#[test]
fn a_read_asks_chunk_0_then_the_rest_of_that_copy() {
    let f = Fake::default();
    f.replies.borrow_mut().extend(right().replies(7));
    let b = read(&f, 1).unwrap();
    assert_eq!(b, right());
    assert_eq!(
        *f.asked.borrow(),
        vec![
            vec![0xD7, 0x13, 1, 0],
            vec![0xD7, 0x13, 1, 1],
            vec![0xD7, 0x13, 1, 2],
            vec![0xD7, 0x13, 1, 3],
        ]
    );
}

#[test]
fn a_copy_that_changes_mid_read_or_an_error_status_fails_the_read() {
    let f = Fake::default();
    let mut replies = left().replies(1);
    replies[2][4] = 2; // another reader took a new copy
    f.replies.borrow_mut().extend(replies);
    assert!(matches!(read(&f, 0), Err(ReadError::Other(e)) if e.contains("changed")));

    for (status, want) in [
        (1, ReadError::Busy),
        (2, ReadError::NoRelay),
        (3, ReadError::RightTimeout),
        (4, ReadError::OldFirmware),
    ] {
        let f = Fake::default();
        let mut r = [0u8; REPORT];
        r[..4].copy_from_slice(&[0xD7, 0x93, status, 1]);
        f.replies.borrow_mut().push_back(r);
        assert_eq!(read(&f, 1).unwrap_err(), want);
    }
}

#[test]
fn without_hello_bit_28_nothing_is_asked() {
    let f = Fake::default();
    let r = read_both(&f, 0);
    assert_eq!(r.left.unwrap_err(), ReadError::NotBuilt);
    assert!(f.asked.borrow().is_empty());
}

#[test]
fn a_version_2_block_carries_the_knob() {
    // #719: bytes 96-103, as widgets/health.h lays them out.
    let b = Block {
        version: 2,
        knob: Some(Knob {
            flags: knob_flags::TURN | knob_flags::PRESS | knob_flags::HELD | knob_flags::ON_APP,
            last: 2,
            events: 12,
            keys: 300,
            drops: 1,
        }),
        ..left()
    };
    let bytes = b.to_bytes();
    assert_eq!(bytes.len(), SIZE_V2);
    assert_eq!(bytes[2], 104);
    assert_eq!(&bytes[96..104], &[0x0F, 2, 12, 0, 0x2C, 0x01, 1, 0]);
    assert_eq!(Block::parse(&bytes).unwrap(), b);
    // Five chunks, the last one 8 bytes; the reader puts them back together.
    let replies = b.replies(3);
    assert_eq!(replies.len(), 5);
    assert_eq!((replies[0][6], replies[0][7]), (5, 104));
    let mut a = Assembler::default();
    let mut got = None;
    for (c, r) in replies.iter().enumerate() {
        got = a.take(0, c as u8, r).unwrap();
    }
    assert_eq!(got.unwrap(), b);
    let rows = rows(&report(b, right()), &Thresholds::default());
    let knob = find(&rows, "Knob");
    assert_eq!(knob.len(), 1);
    assert_eq!(
        knob[0].detail,
        "held by Foxglove, on an app; 12 events, 300 keys, 1 hold dropped"
    );
}

#[test]
fn the_knob_row_says_when_master_volume_is_foxgloves() {
    // #855: bit 4 of byte 96, the PC holds master volume (D7 16).
    let k = Knob {
        flags: knob_flags::TURN | knob_flags::PRESS | knob_flags::HELD | knob_flags::VOLUME,
        last: 1,
        events: 3,
        keys: 0,
        drops: 0,
    };
    assert_eq!(
        knob_words(&k),
        "held by Foxglove, on master (master volume Foxglove's); 3 events, 0 keys, 0 holds dropped"
    );
    let plain = Knob {
        flags: k.flags & !knob_flags::VOLUME,
        ..k
    };
    assert!(knob_words(&plain).starts_with("held by Foxglove, on master;"));
}

#[test]
fn an_older_block_has_no_knob_and_a_shorter_right_reads() {
    // A version 1 block (96 bytes) parses as before, with no knob row; and a version 2 left's
    // D7 93 for an older right says 104 bytes while the block says 96 (docs/health.md).
    let v1 = left();
    assert_eq!(Block::parse(&v1.to_bytes()).unwrap().knob, None);
    let mut padded = right().to_bytes();
    padded.resize(SIZE_V2, 0);
    let r = Block::parse(&padded).unwrap();
    assert_eq!(r.knob, None);
    assert!(find(&rows(&report(left(), r), &Thresholds::default()), "Knob").is_empty());
}

#[test]
fn a_version_3_block_counts_each_transport_and_skipped_is_no_warning() {
    // Bytes 104-111, as widgets/health.h lays them out: USB failed, BLE sent, failed, skipped.
    let b = Block {
        version: 3,
        hid_sent: 112,
        hid_failed: 0,
        knob: Some(Knob::default()),
        hid_transport: Some(HidTransport {
            usb_failed: 0,
            ble_sent: 0,
            ble_failed: 0,
            ble_skipped: 3,
            ..HidTransport::default()
        }),
        ..left()
    };
    let bytes = b.to_bytes();
    assert_eq!(bytes.len(), SIZE_V3);
    assert_eq!(bytes[2], 112);
    assert_eq!(&bytes[104..112], &[0, 0, 0, 0, 0, 0, 3, 0]);
    assert_eq!(Block::parse(&bytes).unwrap(), b);
    // Still five chunks of 24, the last one 16 bytes.
    let replies = b.replies(4);
    assert_eq!(replies.len(), 5);
    assert_eq!((replies[0][6], replies[0][7]), (5, 112));
    let mut a = Assembler::default();
    let mut got = None;
    for (c, r) in replies.iter().enumerate() {
        got = a.take(0, c as u8, r).unwrap();
    }
    assert_eq!(got.unwrap(), b);

    // The 9ddafe5 batch's reading, counted per transport: 3 skipped on Bluetooth, none failed.
    let rows = rows(&report(b.clone(), right()), &Thresholds::default());
    assert_eq!(worst(&rows), Level::Ok, "{rows:#?}");
    let hid = find(&rows, "Reports to the PC");
    assert_eq!(hid.len(), 1, "{rows:#?}");
    assert_eq!(
        hid[0].detail,
        "no problems since start; 3 Bluetooth reports skipped (no PC listening on Bluetooth)"
    );

    // A real refusal stays a warning, said per transport, with the skipped line beside it.
    let failing = Block {
        hid_failed: 2,
        hid_transport: Some(HidTransport {
            usb_failed: 1,
            ble_failed: 1,
            ble_skipped: 1,
            ..HidTransport::default()
        }),
        ..b
    };
    let rows = super::rows(&report(failing, right()), &Thresholds::default());
    let hid: Vec<(Level, &str)> = find(&rows, "Reports to the PC")
        .iter()
        .map(|r| (r.level, r.detail.as_str()))
        .collect();
    assert_eq!(
        hid,
        vec![
            (Level::Warn, "2 reports refused: 1 by USB, 1 by Bluetooth"),
            (
                Level::Ok,
                "1 Bluetooth report skipped (no PC listening on Bluetooth)"
            ),
        ]
    );
}

#[test]
fn an_older_block_has_no_transport_counts() {
    let mut v2 = Block {
        version: 2,
        knob: Some(Knob::default()),
        hid_failed: 3,
        ..left()
    };
    assert_eq!(v2.to_bytes().len(), SIZE_V2);
    assert_eq!(Block::parse(&v2.to_bytes()).unwrap().hid_transport, None);
    v2.knob = None;
    let rows = rows(&report(v2, right()), &Thresholds::default());
    let hid = find(&rows, "Reports to the PC");
    assert_eq!(hid[0].detail, "3 reports refused by USB or Bluetooth");
}

#[test]
fn a_version_4_block_says_why_a_transport_refused() {
    // The d7b5f4f batch's left: Bluetooth sent 50, refused 3 on a link where the PC listens,
    // skipped 3; version 4 adds the last refusal's code (bytes 112-115, int16 each).
    let b = Block {
        version: 4,
        hid_sent: 50,
        hid_failed: 3,
        knob: Some(Knob::default()),
        hid_transport: Some(HidTransport {
            ble_sent: 50,
            ble_failed: 3,
            ble_skipped: 3,
            ble_last_err: -12,
            ..HidTransport::default()
        }),
        ..left()
    };
    let bytes = b.to_bytes();
    assert_eq!(bytes.len(), SIZE_V4);
    assert_eq!(bytes[2], 116);
    assert_eq!(&bytes[112..116], &[0, 0, 0xf4, 0xff]);
    assert_eq!(Block::parse(&bytes).unwrap(), b);
    let replies = b.replies(1);
    assert_eq!(replies.len(), 5);
    assert_eq!((replies[0][6], replies[0][7]), (5, 116));

    let rows = rows(&report(b.clone(), right()), &Thresholds::default());
    let hid: Vec<(Level, &str)> = find(&rows, "Reports to the PC")
        .iter()
        .map(|r| (r.level, r.detail.as_str()))
        .collect();
    assert_eq!(
        hid,
        vec![
            (
                Level::Warn,
                "3 reports refused: 0 by USB, 3 by Bluetooth; Bluetooth's last: -12 ENOMEM, no buffer free"
            ),
            (
                Level::Ok,
                "3 Bluetooth reports skipped (no PC listening on Bluetooth)"
            ),
        ]
    );

    // A version 3 block reads its codes as 0, and the row says no cause.
    let v3 = Block { version: 3, ..b };
    let parsed = Block::parse(&v3.to_bytes()).unwrap();
    assert_eq!(parsed.hid_transport.unwrap().ble_last_err, 0);
    assert_eq!(errno_words(-99), "-99");
}

#[test]
fn usb_reports_while_the_pc_restarts_are_information() {
    // The 11/05 reboot (#741): 6 USB reports refused with -11 EAGAIN while the PC re-enumerated
    // the keyboard, 1 Bluetooth notify refused with -12 ENOMEM.
    let b = Block {
        version: 4,
        hid_sent: 50,
        hid_failed: 7,
        knob: Some(Knob::default()),
        hid_transport: Some(HidTransport {
            usb_failed: 6,
            usb_last_err: USB_NOT_CONFIGURED,
            ble_failed: 1,
            ble_last_err: -12,
            ..HidTransport::default()
        }),
        ..left()
    };
    let rows = rows(&report(b.clone(), right()), &Thresholds::default());
    let hid: Vec<(Level, &str)> = find(&rows, "Reports to the PC")
        .iter()
        .map(|r| (r.level, r.detail.as_str()))
        .collect();
    assert_eq!(
        hid,
        vec![
            (
                Level::Warn,
                "1 report refused: 0 by USB, 1 by Bluetooth; Bluetooth's last: -12 ENOMEM, no buffer free"
            ),
            (
                Level::Ok,
                "6 USB reports not sent while the PC wasn't ready (-11 EAGAIN, not ready yet: a PC restart, sleep or replug)"
            ),
        ]
    );

    // Only the restart's: no warning at all, and the line says so.
    let only_usb = Block {
        hid_failed: 6,
        hid_transport: Some(HidTransport {
            usb_failed: 6,
            usb_last_err: USB_NOT_CONFIGURED,
            ..HidTransport::default()
        }),
        ..b.clone()
    };
    let rows = super::rows(&report(only_usb, right()), &Thresholds::default());
    let hid = find(&rows, "Reports to the PC");
    assert_eq!(hid.len(), 1, "{rows:#?}");
    assert_eq!(hid[0].level, Level::Ok);
    assert!(hid[0]
        .detail
        .starts_with("no problems since start; 6 USB reports not sent"));

    // Another USB code stays a warning.
    let busy = Block {
        hid_failed: 2,
        hid_transport: Some(HidTransport {
            usb_failed: 2,
            usb_last_err: -16,
            ..HidTransport::default()
        }),
        ..b
    };
    let rows = super::rows(&report(busy, right()), &Thresholds::default());
    let hid = find(&rows, "Reports to the PC");
    assert_eq!(hid[0].level, Level::Warn);
    assert_eq!(
        hid[0].detail,
        "2 reports refused: 2 by USB, 0 by Bluetooth; USB's last: -16 EBUSY, busy"
    );
}
