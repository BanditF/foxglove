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
    assert_eq!((w.halves, w.parts.count(1), w.total()), (2, 21, 42));
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
        parts: Vec::new(),
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
    // Under it the lights from before the scene, not the scene's colour.
    assert_eq!(light.state.borrow().hue, 30);
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
        // The lease is the user's own lights: taking them changes nothing on the keyboard, and
        // a hand-back mid-way (the dot stopped) shows them, not a dim or dark board.
        let s = light.state.borrow();
        assert_eq!(
            (s.on, s.hue, s.sat, s.brightness, s.effect.id.as_str()),
            (true, 30, 80, 60, "2")
        );
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
    /// What it answers to each frame.
    shown: Shown,
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
    fn show(&mut self, frame: &[Rgb]) -> Result<Shown, String> {
        let lit = frame.iter().position(|c| *c == white());
        self.log.lock().unwrap().push(format!("show {lit:?}"));
        Ok(self.shown.clone())
    }
    fn rest(&mut self) {
        self.log.lock().unwrap().push("rest".into());
    }
    fn give_back(&mut self) {
        self.log.lock().unwrap().push("give back".into());
    }
}

/// Waits for the worker's status to change (it calls `wake`), with a deadline only as the
/// fallback for a worker that never answers.
fn wait_for(rx: &mpsc::Receiver<()>, status: &Mutex<Status>, done: impl Fn(&Status) -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
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

/// The second desk's one-piece board (`fixtures`): one strip of 61, every LED in turn, and the
/// map found goes back into its profile.
#[test]
fn the_second_desks_board_is_one_strip_of_61() {
    use crate::foxglove::fixtures::second_desk;
    let plank = second_desk::board();
    let mut w = Wizard::new(&plank);
    assert_eq!(
        (w.halves, w.parts.count(0), w.total()),
        (1, second_desk::LAMPS, second_desk::LAMPS)
    );
    assert_eq!(w.frame().len(), second_desk::LAMPS);
    let mut seen = 0;
    while !w.done() {
        let s = w.step().unwrap();
        assert_eq!((s.lamp, s.half, s.index), (seen, 0, seen));
        assert_eq!(w.frame()[seen], white());
        w.place([seen as f32 / 6.0, 1.0]);
        seen += 1;
    }
    assert_eq!(seen, second_desk::LAMPS);
    let r = w.result();
    assert_eq!((r.lamps, r.halves), (61, 1));
    let mut board = plank.clone();
    apply_leds(&mut board, &r);
    assert!(checked(Some(&board)));
    assert_eq!(
        board.lighting.as_ref().unwrap().led_points.len(),
        second_desk::LAMPS
    );
    // Its shipped map is used, and the dot test is asked for once.
    assert_eq!(state(Some(&plank)), MapState::Shipped);
    assert!(placed(Some(&plank)));
}

#[test]
fn a_one_piece_board_is_one_strip() {
    let mut b = corne();
    b.halves = 1;
    let w = Wizard::new(&b);
    assert_eq!((w.halves, w.parts.count(0), w.total()), (1, 42, 42));
    assert_eq!(w.step().unwrap().half, 0);
    // No profile lighting: the Corne's 42 (as one strip on a one-piece board).
    let mut bare = corne();
    bare.lighting = None;
    assert_eq!(Wizard::new(&bare).total(), 42);
    bare.halves = 1;
    let w = Wizard::new(&bare);
    assert_eq!((w.parts.len(), w.total()), (1, 42));
}

/// A split board whose halves differ (30 + 24, the profile's `parts`): the right half starts
/// at lamp 30, and every frame is the board's 54.
#[test]
fn uneven_halves_are_found_half_by_half() {
    let mut b = corne();
    let lighting = b.lighting.as_mut().unwrap();
    (lighting.lamps, lighting.parts) = (54, vec![30, 24]);
    let mut w = Wizard::new(&b);
    assert_eq!(w.frame().len(), 54);
    while w.answers.len() < 30 {
        w.skip();
    }
    let s = w.step().unwrap();
    assert_eq!((s.lamp, s.half, s.index), (30, 1, 0));
    while !w.done() {
        w.place([0.5, 0.5]);
    }
    let r = w.result();
    assert_eq!((r.lamps, &r.parts[..]), (54, &[30, 24][..]));
    assert_eq!(w.part_of(53), 1);
    // The same 54 split another way since: the map no longer fits.
    let mut other = b.clone();
    other.lighting.as_mut().unwrap().parts = vec![27, 27];
    apply_leds(&mut other, &r);
    assert!(!checked(Some(&other)));
    apply_leds(&mut b, &r);
    assert!(checked(Some(&b)));
}

/// A hand-wound clock for the dot test: no real time passes. Running, it jumps straight to the
/// next step whenever the worker has no job waiting; held, time stands still and the worker
/// waits for a job; stepped, each jump waits for the test's [`HandClock::tick`], so the test
/// sees every step whatever the machine's load.
#[derive(Clone)]
struct HandClock {
    now: Arc<Mutex<Instant>>,
    held: Arc<AtomicBool>,
    /// Stepped: the jumps the test has allowed and not yet used (`None`: running freely).
    ticks: Arc<(Mutex<Option<u32>>, std::sync::Condvar)>,
}

impl HandClock {
    fn new() -> HandClock {
        HandClock {
            now: Arc::new(Mutex::new(Instant::now())),
            held: Arc::default(),
            ticks: Arc::default(),
        }
    }

    /// A clock that jumps only when the test ticks it.
    fn stepped() -> HandClock {
        let c = HandClock::new();
        *c.ticks.0.lock().unwrap() = Some(0);
        c
    }

    /// Lets a stepped clock make one jump.
    fn tick(&self) {
        let (count, ready) = &*self.ticks;
        if let Some(n) = count.lock().unwrap().as_mut() {
            *n += 1;
        }
        ready.notify_all();
    }

    /// Stepped: blocks until the test allows a jump, and uses it.
    fn wait_for_tick(&self) {
        let (count, ready) = &*self.ticks;
        let mut n = count.lock().unwrap();
        while *n == Some(0) {
            let (next, waited) = ready
                .wait_timeout(n, std::time::Duration::from_secs(60))
                .unwrap();
            assert!(!waited.timed_out(), "the test never ticked the clock");
            n = next;
        }
        if let Some(n) = n.as_mut() {
            *n -= 1;
        }
    }
}

impl DotClock for HandClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn recv_until(
        &self,
        rx: &mpsc::Receiver<Job>,
        until: Instant,
    ) -> Result<Job, RecvTimeoutError> {
        if self.held.load(Ordering::SeqCst) {
            return rx.recv().map_err(|_| RecvTimeoutError::Disconnected);
        }
        match rx.try_recv() {
            Ok(job) => Ok(job),
            Err(mpsc::TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            Err(mpsc::TryRecvError::Empty) => {
                self.wait_for_tick();
                let mut now = self.now.lock().unwrap();
                *now = (*now).max(until);
                Err(RecvTimeoutError::Timeout)
            }
        }
    }
}

#[test]
fn the_dot_runs_its_rounds_then_gives_the_lights_back_and_a_frame_stops_it() {
    let rec = Recorder::default();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let clock = HandClock::new();
    let started = clock.now();
    let worker = Worker::spawn_on(Box::new(rec.clone()), Box::new(clock.clone()), move || {
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
    // Played: the frame went back to the keyboard (its own lights, not a dark board).
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
            "rest"
        ],
        "two rounds, then the keyboard's own lights"
    );
    // Six steps of 5 ms on the dot's clock: five to the second round's last LED, one to rest.
    assert_eq!(clock.now() - started, std::time::Duration::from_millis(30));
    // Resting, nothing goes out: the keyboard handing back or coming back sends no frame.
    rec.again.lock().unwrap().clone().unwrap()();
    // A new run starts over; a plain frame ends it at once (time stands still meanwhile).
    clock.held.store(true, Ordering::SeqCst);
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
    assert_eq!(
        log[8..],
        ["show Some(0)", "show None", "give back"],
        "nothing sent while resting: the next run, the plain frame, the end"
    );
}

/// A frame the keyboard didn't take (it went away) isn't kept alive: no deadline in the past
/// sending it over and over. The keyboard coming back (Again) or the next step sends it again.
#[test]
fn a_frame_the_keyboard_refused_is_not_kept_alive() {
    /// Takes the first frame, then is gone.
    #[derive(Clone)]
    struct Leaves(Recorder);
    impl Target for Leaves {
        fn take(&mut self, again: Again) -> Result<(), String> {
            self.0.take(again)
        }
        fn show(&mut self, frame: &[Rgb]) -> Result<Shown, String> {
            self.0.show(frame)?;
            if self.0.log.lock().unwrap().len() > 2 {
                return Err("gone".into());
            }
            Ok(Shown {
                hold: Some(std::time::Duration::from_secs(30)),
                missing: None,
            })
        }
        fn rest(&mut self) {}
        fn give_back(&mut self) {
            self.0.give_back();
        }
    }
    let t = Leaves(Recorder::default());
    let log = t.0.log.clone();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let clock = HandClock::new();
    let started = clock.now();
    let worker = Worker::spawn_on(Box::new(t), Box::new(clock.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    let status = worker.status.clone();
    worker.show(Wizard::new(&corne()).frame());
    // Shown, then its keep-alive (the clock runs on to it) fails: nothing more is due, so the
    // worker sleeps on its channel until the end.
    wait_for(&woke, &status, |s| s.error.is_some());
    worker.end_and_wait();
    assert_eq!(
        *log.lock().unwrap(),
        ["take", "show Some(0)", "show Some(0)", "give back"]
    );
    assert_eq!(clock.now() - started, std::time::Duration::from_secs(25));
}

/// Lights that were off before the test stay dark under it (the lease is on, at no brightness).
#[test]
fn lights_that_were_off_stay_dark_under_the_test() {
    let light = Fake::new();
    light.state.borrow_mut().on = false;
    let mut a = Arbiter::default();
    let id = take(&mut a, "kb", &light).unwrap();
    assert_eq!(light.state.borrow().brightness, 0);
    give_back(&mut a, id, &light);
    let s = light.state.borrow();
    assert_eq!((s.on, s.brightness), (false, 60), "as they were");
}

/// The drawing's dot moves when the keyboard has the frame, on the same clock as the dot: the
/// step the page shows is always the one the keyboard was last sent.
#[test]
fn the_page_shows_the_step_the_keyboard_has() {
    let rec = Recorder::default();
    let log = rec.log.clone();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    // Stepped: the dot moves only when the test has looked at the page, so no step goes by
    // unseen (a free-running clock let the whole run finish before the test first looked).
    let clock = HandClock::stepped();
    let worker = Worker::spawn_on(Box::new(rec), Box::new(clock.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    let status = worker.status.clone();
    let frames: Vec<Vec<Rgb>> = (0..4)
        .map(|i| {
            let mut f = vec![Rgb { r: 0, g: 0, b: 0 }; 4];
            f[i] = white();
            f
        })
        .collect();
    worker.play(Play {
        frames,
        every: std::time::Duration::from_millis(700),
        rounds: 1,
    });
    let mut seen = Vec::new();
    // Every time the page wakes, the step it shows has gone to the keyboard: the page is never
    // ahead of it. The test looks, then lets the dot take one step.
    while !status.lock().unwrap().played {
        assert!(woke
            .recv_timeout(std::time::Duration::from_secs(60))
            .is_ok());
        let s = status.lock().unwrap().clone();
        let sent: Option<usize> = log.lock().unwrap().iter().rev().find_map(|l| {
            l.strip_prefix("show Some(")
                .and_then(|n| n.trim_end_matches(')').parse().ok())
        });
        if let Some(step) = s.step {
            assert!(
                sent.is_some_and(|n| n >= step),
                "page {step}, keyboard {sent:?}"
            );
            if seen.last() != Some(&step) {
                seen.push(step);
                clock.tick();
            }
        }
    }
    assert_eq!(seen, [0, 1, 2, 3], "every step, in order");
    worker.end_and_wait();
}

/// A frame held for a while (a step of Find my LEDs, the user looking for the LED) goes again
/// before the keyboard's own hold runs out, so it never hands the lights back mid-way; the
/// keep-alive stops with the wizard.
#[test]
fn a_held_frame_goes_again_before_the_keyboard_lets_go() {
    let rec = Recorder {
        shown: Shown {
            hold: Some(std::time::Duration::from_secs(30)),
            missing: None,
        },
        ..Default::default()
    };
    let log = rec.log.clone();
    let (tx, woke) = mpsc::channel();
    let tx = Mutex::new(tx);
    let clock = HandClock::new();
    let started = clock.now();
    let worker = Worker::spawn_on(Box::new(rec), Box::new(clock.clone()), move || {
        let _ = tx.lock().unwrap().send(());
    });
    let status = worker.status.clone();
    let frame = Wizard::new(&corne()).frame();
    worker.show(frame.clone());
    // The hand-wound clock jumps to each keep-alive at once: three sends, then hold it.
    while log.lock().unwrap().len() < 4 {
        assert!(woke
            .recv_timeout(std::time::Duration::from_secs(60))
            .is_ok());
    }
    clock.held.store(true, Ordering::SeqCst);
    worker.end_and_wait();
    let log = log.lock().unwrap();
    assert_eq!(log[0], "take");
    assert!(log[1..log.len() - 1].iter().all(|l| l == "show Some(0)"));
    assert_eq!(log.last().map(String::as_str), Some("give back"));
    // Each one 25 s after the last: 5 s inside the keyboard's 30.
    let sends = (log.len() - 2) as u32;
    assert_eq!(
        clock.now() - started,
        std::time::Duration::from_secs(25) * (sends - 1)
    );
    assert_eq!(status.lock().unwrap().shown.as_ref(), Some(&frame));
    assert_eq!(
        keep_alive(std::time::Duration::from_secs(6)),
        std::time::Duration::from_secs(3)
    );
}

/// What the keyboard's answer means: its hold, and a split's other half that doesn't draw.
#[test]
fn the_keyboards_answer_gives_the_hold_and_a_missing_half() {
    use foxglove_link::protocol::{FrameReply, FrameRight};
    let reply = |timeout_s, right| FrameReply {
        status: 0,
        owner: FrameReply::OWNER_HOST_LINK,
        timeout_s,
        right,
        event: false,
    };
    let s = shown(&reply(30, FrameRight::Draws), 2);
    assert_eq!(s.hold, Some(std::time::Duration::from_secs(30)));
    assert!(s.missing.is_none());
    assert!(
        shown(&reply(0, FrameRight::Draws), 2).hold.is_none(),
        "never lets go"
    );
    assert!(shown(&reply(30, FrameRight::NoLink), 2).missing.is_some());
    assert!(shown(&reply(30, FrameRight::Unsupported), 2)
        .missing
        .is_some());
    // A one-piece board has no other half to miss (its firmware says "no link").
    assert!(shown(&reply(30, FrameRight::NoLink), 1).missing.is_none());
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
