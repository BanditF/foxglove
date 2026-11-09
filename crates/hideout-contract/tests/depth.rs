//! Hostile nesting must come back as an error, never a stack overflow (#941).
//! Every case runs on a 256 KiB stack: an overflow aborts the whole test binary.

use hideout_contract::config::Config;
use hideout_contract::module::{Context, Module};
use hideout_contract::panel::{parse_message, MAX_MESSAGE_NESTING};
use hideout_contract::Version;

const DEEP: usize = 100_000;

fn small_stack<F: FnOnce() + Send + 'static>(f: F) {
    on_stack(256 * 1024, f)
}

fn on_stack<F: FnOnce() + Send + 'static>(bytes: usize, f: F) {
    std::thread::Builder::new()
        .stack_size(bytes)
        .spawn(f)
        .unwrap()
        .join()
        .expect("thread panicked");
}

fn objects(n: usize) -> String {
    format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n))
}

fn arrays(n: usize) -> String {
    format!("{}{}", "[".repeat(n), "]".repeat(n))
}

#[test]
fn parse_message_refuses_deep_objects_and_arrays() {
    small_stack(|| {
        for text in [
            objects(DEEP),
            arrays(DEEP),
            objects(MAX_MESSAGE_NESTING + 1),
        ] {
            let e = parse_message(&text).unwrap_err();
            assert!(e.starts_with("ui.limits:"), "{e}");
        }
    });
}

#[test]
fn parse_message_accepts_exactly_the_cap() {
    // 1 MiB: the Windows main thread. A debug build's frames are large.
    on_stack(1024 * 1024, || {
        for text in [objects(MAX_MESSAGE_NESTING), arrays(MAX_MESSAGE_NESTING)] {
            let v = parse_message(&text).expect("at the cap");
            drop(v);
        }
    });
}

#[test]
fn deep_brackets_inside_strings_do_not_count() {
    small_stack(|| {
        let text = format!("{{\"s\":\"{}\"}}", "[".repeat(DEEP));
        assert!(parse_message(&text).is_ok());
    });
}

#[test]
fn toml_strings_and_comments_do_not_count() {
    small_stack(|| {
        let text = format!(
            "# {}
a = \"{}\"
b = '{}'
c = \"\"\"
{}\"\"\"
",
            "[".repeat(DEEP),
            "[{".repeat(DEEP),
            "[".repeat(DEEP),
            "{".repeat(DEEP)
        );
        // Whatever else is wrong with it, it is not refused for nesting.
        let e = Module::parse(&text, &Context::default()).unwrap_err();
        assert!(!e.message.contains("nests deeper"), "{}", e.message);
    });
}

#[test]
fn toml_readers_error_on_deep_inline_nesting() {
    small_stack(|| {
        let arr = format!("x = {}{}\n", "[".repeat(DEEP), "]".repeat(DEEP));
        let tab = format!("x = {}1{}\n", "{a=".repeat(DEEP), "}".repeat(DEEP));
        let ctx = Context::default();
        for text in [arr, tab] {
            let e = Config::parse(&text).unwrap_err();
            assert!(e.message.contains("nests deeper"), "{}", e.message);
            assert!(Module::parse(&text, &ctx).is_err());
            assert!(Module::parse_legacy(
                &text,
                Version {
                    major: 1,
                    minor: 14
                }
            )
            .is_err());
            assert!(Config::parse(&text).is_err());
        }
    });
}
