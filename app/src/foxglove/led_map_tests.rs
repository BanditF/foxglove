use super::*;
use std::cell::RefCell;
use std::sync::atomic::AtomicU32;

use foxglove_devices::{Effect, LightingState};

fn corne() -> Board {
    Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap()
}

fn white() -> Rgb {
    let [r, g, b] = LIT;
    Rgb { r, g, b }
}

#[test]
fn the_wizard_lights_each_led_in_turn_and_back_undoes() {
    let mut w = Wizard::new(&corne());
    assert_eq!((w.halves, w.per_half, w.total()), (2, 21, 42));
    let s = w.step().unwrap();
    assert_eq!((s.lamp, s.half, s.index), (0, 0, 0));
    let f = w.frame();
    assert_eq!(f.len(), 42);
    assert_eq!(f[0], white());
    assert_eq!(
        f.iter().filter(|c| **c == white()).count(),
        1,
        "one LED lit"
    );

    w.place([1.0, 2.0]);
    w.skip();
    assert_eq!(w.step().unwrap().lamp, 2);
    assert_eq!(w.frame()[2], white());
    w.back();
    assert_eq!(
        w.step().unwrap().lamp,
        1,
        "Back lights the one before again"
    );
    assert_eq!(w.answers, vec![Some([1.0, 2.0])]);
    // A point that isn't one is no answer.
    w.place([f32::NAN, 1.0]);
    assert_eq!(w.answers.len(), 1);

    // The right half's first LED is lamp 21.
    while w.answers.len() < 21 {
        w.skip();
    }
    let s = w.step().unwrap();
    assert_eq!((s.lamp, s.half, s.index), (21, 1, 0));
    while !w.done() {
        w.place([0.5, 0.5]);
    }
    assert!(w.frame().iter().all(|c| *c == Rgb { r: 0, g: 0, b: 0 }));
    // Done: more answers are ignored, Back still works.
    w.place([9.0, 9.0]);
    w.skip();
    assert_eq!(w.answers.len(), 42);
    w.back();
    assert!(!w.done());
}

#[test]
fn the_map_keeps_only_the_leds_seen_and_goes_back_into_the_profile() {
    let mut w = Wizard::new(&corne());
    w.place([0.12345, 0.5]);
    w.skip();
    w.place([3.0, 4.0]);
    let r = w.result();
    assert_eq!((r.lamps, r.halves), (42, 2));
    assert_eq!(
        r.leds,
        vec![
            Seen {
                lamp: 0,
                x: 0.123,
                y: 0.5
            },
            Seen {
                lamp: 2,
                x: 3.0,
                y: 4.0
            }
        ]
    );
    let points = r.points();
    assert_eq!(points.len(), 42);
    assert_eq!(points[1], None);
    assert_eq!(points[2], Some([3.0, 4.0]));

    let mut board = corne();
    assert_ne!(board.lighting.as_ref().unwrap().led_map, LED_MAP_CHECKED);
    apply_leds(&mut board, &r);
    let l = board.lighting.as_ref().unwrap();
    assert_eq!(l.led_map, LED_MAP_CHECKED);
    assert_eq!(l.led_points, points);
    assert!(checked(Some(&board)));
    assert!(placed(Some(&board)), "frames, not the firmware's table");
    // The shipped map places them too; no board doesn't.
    assert!(placed(Some(&corne())) && !placed(None));

    // A map found with another lamp count doesn't fit the profile any more.
    let mut other = corne();
    apply_leds(
        &mut other,
        &BoardLeds {
            lamps: 40,
            ..r.clone()
        },
    );
    assert!(!checked(Some(&other)));
    // A lamp past the end, or a broken number in the store, is dropped.
    let odd = BoardLeds {
        lamps: 2,
        halves: 2,
        leds: vec![
            Seen {
                lamp: 7,
                x: 1.0,
                y: 1.0,
            },
            Seen {
                lamp: 1,
                x: f32::INFINITY,
                y: 1.0,
            },
        ],
    };
    assert_eq!(odd.points(), vec![None, None]);
}

#[test]
fn the_map_survives_the_store() {
    crate::foxglove::store::test_config_dir();
    let mut w = Wizard::new(&corne());
    w.place([2.5, 1.25]);
    w.skip();
    let mut all = Saved::default();
    all.boards.insert("test-ledmap-board".into(), w.result());
    crate::foxglove::store::save(SECTION, &all).unwrap();
    let back = saved("test-ledmap-board").unwrap();
    assert_eq!(back, w.result());
    let mut board = corne();
    board.id = "test-ledmap-board".into();
    apply(&mut board);
    assert_eq!(board.lighting.unwrap().led_points[0], Some([2.5, 1.25]));
}

/// Lights in memory: counts releases, keeps the last frame.
struct Fake {
    state: RefCell<LightingState>,
    releases: AtomicU32,
    frame: RefCell<Vec<Rgb>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            state: RefCell::new(LightingState {
                on: true,
                hue: 30,
                sat: 80,
                brightness: 60,
                effect: Effect {
                    id: "2".into(),
                    params: vec![],
                },
                speed: 3,
                settled: true,
            }),
            releases: AtomicU32::new(0),
            frame: RefCell::new(Vec::new()),
        }
    }
}

impl Lighting for Fake {
    fn layout(&self) -> foxglove_devices::Result<Vec<foxglove_devices::Zone>> {
        Ok(vec![])
    }
    fn state(&self) -> foxglove_devices::Result<LightingState> {
        Ok(self.state.borrow().clone())
    }
    fn set_on(&self, on: bool) -> foxglove_devices::Result<()> {
        self.state.borrow_mut().on = on;
        Ok(())
    }
    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> foxglove_devices::Result<()> {
        let mut s = self.state.borrow_mut();
        (s.hue, s.sat, s.brightness) = (hue, sat, brightness);
        Ok(())
    }
    fn set_effect(&self, effect: Effect) -> foxglove_devices::Result<()> {
        self.state.borrow_mut().effect = effect;
        Ok(())
    }
    fn set_frame(&self, frame: &[Rgb]) -> foxglove_devices::Result<()> {
        *self.frame.borrow_mut() = frame.to_vec();
        Ok(())
    }
    fn release(&self) -> foxglove_devices::Result<()> {
        self.releases.fetch_add(1, Ordering::Relaxed);
        self.frame.borrow_mut().clear();
        Ok(())
    }
}

#[test]
fn the_wizard_takes_the_lights_over_a_scene_and_the_scene_draws_again_after() {
    let light = Fake::new();
    let mut a = Arbiter::default();
    // A scene holds the keyboard (low lease) and has drawn a frame.
    let scene = a
        .add(
            Lease {
                id: 0,
                conn: u64::MAX - 1,
                device: "kb".into(),
                priority: Priority::Low,
                hue: 200,
                sat: 50,
                brightness: 70,
                effect: "0".into(),
            },
            &light,
        )
        .unwrap();
    let asked = Arc::new(AtomicU32::new(0));
    let asked2 = asked.clone();
    a.draws_frames(scene, move || {
        asked2.fetch_add(1, Ordering::Relaxed);
    });
    a.frame_sent("kb");
    let before = asked.load(Ordering::Relaxed);

    // The wizard's lease wins: the scene's frame is handed back first.
    let id = take(&mut a, "kb", &light).unwrap();
    assert!(a.shows(id) && !a.shows(scene));
    assert_eq!(light.releases.load(Ordering::Relaxed), 1);
    light.set_frame(&[white()]).unwrap();

    // Giving back: the wizard's frame goes, its lease ends, the scene is asked to draw again.
    give_back(&mut a, id, &light);
    assert_eq!(light.releases.load(Ordering::Relaxed), 2);
    assert!(light.frame.borrow().is_empty());
    assert!(a.shows(scene));
    assert_eq!(
        asked.load(Ordering::Relaxed),
        before + 1,
        "the scene draws again"
    );
    // The scene still draws frames: its own hand-back still works.
    a.frame_sent("kb");
    assert!(a.holds("kb"));
}

#[test]
fn with_no_scene_the_lights_from_before_come_back() {
    let light = Fake::new();
    let mut a = Arbiter::default();
    let id = take(&mut a, "kb", &light).unwrap();
    {
        let s = light.state.borrow();
        assert_eq!((s.sat, s.effect.id.as_str()), (0, "0"), "dim white, solid");
    }
    give_back(&mut a, id, &light);
    let s = light.state.borrow();
    assert_eq!(
        (s.on, s.hue, s.sat, s.brightness, s.effect.id.as_str()),
        (true, 30, 80, 60, "2")
    );
}

/// A keyboard for the worker, recording what it was asked.
#[derive(Clone, Default)]
struct Recorder {
    log: Arc<Mutex<Vec<String>>>,
    again: Arc<Mutex<Option<Again>>>,
    refuse: bool,
}

impl Target for Recorder {
    fn take(&mut self, again: Again) -> Result<(), String> {
        if self.refuse {
            return Err("refused".into());
        }
        *self.again.lock().unwrap() = Some(again);
        self.log.lock().unwrap().push("take".into());
        Ok(())
    }
    fn show(&mut self, frame: &[Rgb]) -> Result<(), String> {
        let lit = frame.iter().position(|c| *c == white());
        self.log.lock().unwrap().push(format!("show {lit:?}"));
        Ok(())
    }
    fn give_back(&mut self) {
        self.log.lock().unwrap().push("give back".into());
    }
}

/// Waits for the worker's status to change (it calls `wake`), with a deadline.
fn wait_for(rx: &mpsc::Receiver<()>, status: &Mutex<Status>, done: impl Fn(&Status) -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done(&status.lock().unwrap()) {
        let left = until.saturating_duration_since(std::time::Instant::now());
        assert!(rx.recv_timeout(left).is_ok(), "the worker went quiet");
    }
}

#[test]
fn the_worker_holds_the_lights_until_the_wizard_ends() {
    let rec = Recorder::default();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::spawn(Box::new(rec.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    let mut w = Wizard::new(&corne());
    worker.show(w.frame());
    let status = worker.status.clone();
    wait_for(&woke, &status, |s| s.shown.is_some());
    w.place([1.0, 1.0]);
    worker.show(w.frame());
    let frame = w.frame();
    wait_for(&woke, &status, |s| s.shown.as_ref() == Some(&frame));
    // The keyboard handed the frame back (its timeout): the same LED again.
    let again = rec.again.lock().unwrap().clone().unwrap();
    again();
    wait_for(&woke, &status, |_| {
        rec.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| *l == "show Some(1)")
            .count()
            == 2
    });
    worker.end_and_wait();
    assert_eq!(
        *rec.log.lock().unwrap(),
        vec![
            "take",
            "show Some(0)",
            "show Some(1)",
            "show Some(1)",
            "give back"
        ]
    );
}

#[test]
fn a_refused_take_shows_the_error_and_gives_nothing_back() {
    let rec = Recorder {
        refuse: true,
        ..Default::default()
    };
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::spawn(Box::new(rec.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    worker.show(Wizard::new(&corne()).frame());
    let status = worker.status.clone();
    wait_for(&woke, &status, |s| s.error.is_some());
    worker.end_and_wait();
    assert!(rec.log.lock().unwrap().is_empty());
}

#[test]
fn only_the_newest_step_goes_out_and_an_end_wins() {
    let a = Job::Show(vec![white()]);
    let b = Job::Show(vec![]);
    assert_eq!(newest(a, b), Job::Show(vec![]));
    assert_eq!(
        newest(Job::Show(vec![white()]), Job::Again),
        Job::Show(vec![white()])
    );
    assert_eq!(newest(Job::Again, Job::Show(vec![])), Job::Show(vec![]));
    assert_eq!(newest(Job::End, Job::Show(vec![])), Job::End);
    assert_eq!(newest(Job::Again, Job::End), Job::End);
    let play = || {
        Job::Play(Play {
            frames: vec![vec![white()]],
            every: std::time::Duration::from_millis(1),
            rounds: 1,
        })
    };
    assert_eq!(newest(Job::Show(vec![]), play()), play());
    assert_eq!(newest(play(), Job::Again), play());
    assert_eq!(newest(play(), Job::Show(vec![])), Job::Show(vec![]));
}

#[test]
fn a_one_piece_board_is_one_strip() {
    let mut b = corne();
    b.halves = 1;
    let w = Wizard::new(&b);
    assert_eq!((w.halves, w.per_half, w.total()), (1, 42, 42));
    assert_eq!(w.step().unwrap().half, 0);
    // No profile lighting: the Corne's 42.
    let mut bare = corne();
    bare.lighting = None;
    assert_eq!(Wizard::new(&bare).total(), 42);
}

#[test]
fn the_dot_runs_its_rounds_then_goes_dark_and_a_frame_stops_it() {
    let rec = Recorder::default();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::spawn(Box::new(rec.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    let frames: Vec<Vec<Rgb>> = (0..3)
        .map(|i| {
            let mut f = vec![Rgb { r: 0, g: 0, b: 0 }; 3];
            f[i] = white();
            f
        })
        .collect();
    let status = worker.status.clone();
    worker.play(Play {
        frames: frames.clone(),
        every: std::time::Duration::from_millis(5),
        rounds: 2,
    });
    wait_for(&woke, &status, |s| s.played);
    assert_eq!(status.lock().unwrap().step, None);
    let shown: Vec<String> = rec.log.lock().unwrap().clone();
    assert_eq!(
        shown,
        vec![
            "take",
            "show Some(0)",
            "show Some(1)",
            "show Some(2)",
            "show Some(0)",
            "show Some(1)",
            "show Some(2)",
            "show None"
        ],
        "two rounds, then all off"
    );
    // A new run starts over; a plain frame ends it at once.
    worker.play(Play {
        frames,
        every: std::time::Duration::from_secs(60),
        rounds: 1,
    });
    wait_for(&woke, &status, |s| s.step == Some(0) && !s.played);
    worker.show(vec![Rgb { r: 0, g: 0, b: 0 }; 3]);
    wait_for(&woke, &status, |s| s.step.is_none());
    worker.end_and_wait();
    let log = rec.log.lock().unwrap();
    assert_eq!(log.last().map(String::as_str), Some("give back"));
    assert_eq!(log.iter().filter(|l| *l == "take").count(), 1);
}

#[test]
fn a_shipped_map_is_used_and_asked_about_a_checked_one_is_not() {
    let mut b = corne();
    assert_eq!(state(Some(&b)), MapState::Shipped);
    // A shipped map places the LEDs: firmware without EFFECT LAMPS gets frames, as the room has
    // them, not its own built-in table.
    assert!(placed(Some(&b)));
    b.lighting.as_mut().unwrap().led_map = LED_MAP_CHECKED.into();
    assert_eq!(state(Some(&b)), MapState::Checked);
    b.lighting.as_mut().unwrap().led_map = "provisional".into();
    assert_eq!(state(Some(&b)), MapState::Unknown);
    assert!(!placed(Some(&b)), "a provisional map isn't used");
    assert_eq!(state(None), MapState::Unknown);
    // A saved map (found, or confirmed by the dot test) marks it checked.
    let mut b = corne();
    let c = super::super::led_map_confirm::Confirm::new(&b).unwrap();
    apply_leds(&mut b, &c.result());
    assert_eq!(state(Some(&b)), MapState::Checked);
}
