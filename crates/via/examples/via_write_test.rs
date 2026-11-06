//! The real-board VIA/Vial write test (#723), window-free: one step at a time, each one
//! read before, write, read after, restore, read again. The test plan is
//! `docs/test-lab/via-writes-real-board.md`.
//!
//! ```text
//! cargo run -p foxglove-via --features via-writes --example via_write_test -- \
//!     <vvvv:pppp> [--definition FILE] [--generic] <step> [--apply] [--leave]
//!
//! steps:
//!   info                                  reads only: firmware, layers, lighting, options
//!   keycode LAYER ROW COL CODE            one key, then put back
//!   lighting HUE SAT BRIGHTNESS EFFECT SPEED   device-layer units (0-359, %, %, QMK, 0-255)
//!   layout VALUE                          the layout options bitfield, then put back
//!   save-unchanged                        CUSTOM_SAVE of the lighting as it is (no change)
//! ```
//!
//! `--leave` skips the put-back (for the replug check) and prints the command that puts it back.
//! Without `--apply` nothing is written: the step only reads and prints what it would write.
//! With `--apply`, writes need a `via-writes` build and `FOXGLOVE_VIA_WRITES=1`; without them
//! every write is refused before it reaches the keyboard. Exit 0 = PASS, 1 = FAIL, 2 = refused
//! or a usage error. Never run it against the daily driver without the user's OK.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

/// `--leave`: write and read back, but don't put back (for the replug check); the output says
/// how to put it back.
static LEAVE: AtomicBool = AtomicBool::new(false);

fn leave() -> bool {
    LEAVE.load(Ordering::Relaxed)
}

use foxglove_devices::{Backend, Binding, Effect, Keymap, Lighting};
use foxglove_via::hid::{HidTransport, ViaBackend};
use foxglove_via::{Definition, ViaDevice};

const USAGE: &str = "usage: via_write_test <vvvv:pppp> [--definition FILE] [--generic] \
                     <info | keycode L R C CODE | lighting H S B EFFECT SPEED | layout VALUE | \
                     save-unchanged> [--apply] [--leave]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(true) => {
            println!("RESULT PASS");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("RESULT FAIL");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("refused: {e}");
            ExitCode::from(2)
        }
    }
}

fn number(s: &str) -> Result<u32, String> {
    let parsed = match s.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse(),
    };
    parsed.map_err(|_| format!("not a number: {s}"))
}

fn run(mut args: Vec<String>) -> Result<bool, String> {
    let apply = take_flag(&mut args, "--apply");
    let generic = take_flag(&mut args, "--generic");
    LEAVE.store(take_flag(&mut args, "--leave"), Ordering::Relaxed);
    let definition = match args.iter().position(|a| a == "--definition") {
        Some(i) if i + 1 < args.len() => {
            let path = args.remove(i + 1);
            args.remove(i);
            Some(Definition::load(std::path::Path::new(&path))?)
        }
        Some(_) => return Err(USAGE.into()),
        None => None,
    };
    let [model, step, rest @ ..] = args.as_slice() else {
        return Err(USAGE.into());
    };
    let (vid, pid) = model
        .split_once(':')
        .and_then(|(v, p)| {
            Some((
                u16::from_str_radix(v, 16).ok()?,
                u16::from_str_radix(p, 16).ok()?,
            ))
        })
        .ok_or(USAGE)?;
    if apply && !foxglove_via::writes_enabled() {
        return Err(format!(
            "--apply needs a via-writes build and {}=1; nothing was written",
            foxglove_via::WRITES_ENV
        ));
    }

    let mut backend = ViaBackend::new(definition.into_iter().collect());
    if generic {
        backend = backend.generic();
    }
    if apply {
        backend = backend.allow_writes();
    }
    let found = backend.discover().map_err(|e| e.to_string())?;
    let mut boards = found.iter().filter(|i| (i.id.vid, i.id.pid) == (vid, pid));
    let (Some(info), None) = (boards.next(), boards.next()) else {
        return Err(format!(
            "exactly one {vid:04x}:{pid:04x} on USB Raw HID needed (found {})",
            found
                .iter()
                .filter(|i| (i.id.vid, i.id.pid) == (vid, pid))
                .count()
        ));
    };
    let dev = backend.open_via(info).map_err(|e| e.to_string())?;
    println!("BOARD {} {:?}", info.name, dev.summary());
    println!(
        "WRITES {}",
        if apply {
            "ON (applying)"
        } else {
            "off (dry run)"
        }
    );

    let nums = |n: usize| -> Result<Vec<u32>, String> {
        if rest.len() != n {
            return Err(USAGE.into());
        }
        rest.iter().map(|s| number(s)).collect()
    };
    match step.as_str() {
        "info" => info_step(&dev),
        "keycode" => {
            let v = nums(4)?;
            keycode_step(&dev, apply, v[0], v[1] as u16, v[2] as u16, v[3])
        }
        "lighting" => {
            let v = nums(5)?;
            lighting_step(
                &dev,
                apply,
                v[0] as u16,
                v[1] as u8,
                v[2] as u8,
                v[3],
                v[4] as u8,
            )
        }
        "layout" => layout_step(&dev, apply, nums(1)?[0]),
        "save-unchanged" => save_step(&dev, apply),
        _ => Err(USAGE.into()),
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

fn e(err: foxglove_devices::DeviceError) -> String {
    err.to_string()
}

fn info_step(dev: &ViaDevice<HidTransport>) -> Result<bool, String> {
    println!("PROBE {:?}", dev.probe);
    if let Ok(raw) = dev.raw_lighting() {
        println!("LIGHTING {raw:?}");
    }
    if let Ok(v) = dev.read_layout_options() {
        println!("LAYOUT_OPTIONS {v:#x}");
    }
    if let Ok(map) = dev.read_keymap() {
        println!(
            "KEYMAP {} keycodes, checksum {:#010x}",
            map.len(),
            checksum(&map)
        );
    }
    Ok(true)
}

fn checksum(map: &[u16]) -> u32 {
    map.iter().fold(0x811C_9DC5u32, |h, &c| {
        (h ^ u32::from(c)).wrapping_mul(0x0100_0193)
    })
}

fn qmk(code: u32) -> Binding {
    Binding {
        kind: "qmk".into(),
        params: vec![code],
        label: None,
    }
}

fn keycode_step(
    dev: &ViaDevice<HidTransport>,
    apply: bool,
    layer: u32,
    row: u16,
    col: u16,
    code: u32,
) -> Result<bool, String> {
    let map_before = dev.read_keymap().map_err(e)?;
    let before = Keymap::binding(dev, layer, row, col).map_err(e)?;
    println!(
        "READ-BEFORE key {layer},{row},{col} = {:#06x}",
        before.params[0]
    );
    println!(
        "READ-BEFORE keymap checksum {:#010x}",
        checksum(&map_before)
    );
    if !apply {
        println!("WOULD-WRITE key {layer},{row},{col} = {code:#06x}, then put back");
        return Ok(true);
    }
    println!("WRITE key {layer},{row},{col} = {code:#06x}");
    if let Err(err) = Keymap::set_binding(dev, layer, row, col, qmk(code)) {
        // Refused before it went out (or failed): nothing to put back.
        println!("WRITE-ERROR {err}");
        println!("NOTHING-WRITTEN no put-back");
        return Ok(false);
    }
    let after = Keymap::binding(dev, layer, row, col).map_err(e)?;
    println!("READ-AFTER key = {:#06x}", after.params[0]);
    let map_after = dev.read_keymap().map_err(e)?;
    let changed = map_before
        .iter()
        .zip(&map_after)
        .filter(|(a, b)| a != b)
        .count();
    println!("READ-AFTER keymap: {changed} keycode(s) changed");
    let ok = after.params == [code] && changed <= 1;
    if leave() {
        println!(
            "LEFT key {layer},{row},{col} not restored; put it back with: \
             keycode {layer} {row} {col} {:#06x} --apply --leave",
            before.params[0]
        );
        return Ok(ok);
    }
    println!(
        "RESTORE key {layer},{row},{col} = {:#06x}",
        before.params[0]
    );
    Keymap::set_binding(dev, layer, row, col, before.clone()).map_err(e)?;
    let restored = Keymap::binding(dev, layer, row, col).map_err(e)?;
    let map_restored = dev.read_keymap().map_err(e)?;
    println!(
        "READ-RESTORED key = {:#06x}, keymap checksum {:#010x}",
        restored.params[0],
        checksum(&map_restored)
    );
    Ok(ok && restored == before && map_restored == map_before)
}

fn lighting_step(
    dev: &ViaDevice<HidTransport>,
    apply: bool,
    hue: u16,
    sat: u8,
    brightness: u8,
    effect: u32,
    speed: u8,
) -> Result<bool, String> {
    let lighting = Lighting::state(dev).map_err(e)?;
    let raw_before = dev.raw_lighting().map_err(e)?;
    println!("READ-BEFORE {lighting:?}");
    println!("READ-BEFORE raw {raw_before:?}");
    if !apply {
        println!(
            "WOULD-WRITE colour {hue}/{sat}/{brightness}, effect {effect}, speed {speed} \
             (RAM only), then put back"
        );
        return Ok(true);
    }
    let mut ok = true;
    println!("WRITE colour {hue}/{sat}/{brightness}, effect {effect}, speed {speed} (RAM only)");
    let wrote = Lighting::set_color(dev, hue, sat, brightness)
        .and_then(|_| {
            Lighting::set_effect(
                dev,
                Effect {
                    id: effect.to_string(),
                    params: vec![],
                },
            )
        })
        .and_then(|_| match Lighting::set_speed(dev, speed) {
            Err(foxglove_devices::DeviceError::Unsupported) => Ok(()), // a backlight
            other => other,
        });
    if let Err(err) = &wrote {
        println!("WRITE-ERROR {err}");
        ok = false;
    } else {
        let after = Lighting::state(dev).map_err(e)?;
        println!("READ-AFTER {after:?}");
        println!("READ-AFTER raw {:?}", dev.raw_lighting().map_err(e)?);
        ok &= after.effect.id == effect.to_string();
        // Percent and degrees round through QMK's 0-255.
        ok &= after.brightness.abs_diff(brightness) <= 1;
    }
    if leave() {
        println!("LEFT lighting in RAM; after a replug, `info` must show LIGHTING {raw_before:?}");
        return Ok(ok);
    }
    println!("RESTORE raw {raw_before:?} (RAM only)");
    dev.restore_raw_lighting(&raw_before).map_err(e)?;
    let raw_restored = dev.raw_lighting().map_err(e)?;
    println!("READ-RESTORED raw {raw_restored:?}");
    Ok(ok && raw_restored == raw_before)
}

fn layout_step(dev: &ViaDevice<HidTransport>, apply: bool, value: u32) -> Result<bool, String> {
    let before = dev.read_layout_options().map_err(e)?;
    println!("READ-BEFORE layout options {before:#x}");
    if !apply {
        println!("WOULD-WRITE layout options {value:#x}, then put back");
        return Ok(true);
    }
    let mut ok = true;
    println!("WRITE layout options {value:#x}");
    match dev.set_layout_options(value) {
        Ok(()) => {
            let after = dev.read_layout_options().map_err(e)?;
            println!("READ-AFTER layout options {after:#x}");
            ok &= after == value;
        }
        Err(err) => {
            println!("WRITE-ERROR {err}");
            println!("NOTHING-WRITTEN no put-back");
            return Ok(false);
        }
    }
    if leave() {
        println!("LEFT layout options not restored; put them back with: layout {before:#x} --apply --leave");
        return Ok(ok);
    }
    println!("RESTORE layout options {before:#x}");
    dev.set_layout_options(before).map_err(e)?;
    let restored = dev.read_layout_options().map_err(e)?;
    println!("READ-RESTORED layout options {restored:#x}");
    Ok(ok && restored == before)
}

fn save_step(dev: &ViaDevice<HidTransport>, apply: bool) -> Result<bool, String> {
    let before = dev.raw_lighting().map_err(e)?;
    println!("READ-BEFORE raw {before:?}");
    if !apply {
        println!("WOULD-WRITE CUSTOM_SAVE of the unchanged lighting");
        return Ok(true);
    }
    println!("WRITE CUSTOM_SAVE (lighting unchanged)");
    dev.save_lighting().map_err(e)?;
    let after = dev.raw_lighting().map_err(e)?;
    println!("READ-AFTER raw {after:?}");
    Ok(after == before)
}
