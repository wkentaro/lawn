use super::*;
use std::sync::{Arc, Mutex};

const SPACING: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Fake {
    document: Mutex<(Value, u64)>,
}

impl Board for Fake {
    fn load(&self) -> Result<(Value, u64), String> {
        Ok(self.document.lock().unwrap().clone())
    }

    fn save(&self, value: &Value, version: u64) -> Result<bool, String> {
        let mut document = self.document.lock().unwrap();
        if document.1 != version {
            return Ok(false);
        }
        *document = (value.clone(), version + 1);
        Ok(true)
    }
}

type Event = (Duration, Box<dyn FnOnce() + Send>);

struct Clock {
    now: Arc<Mutex<Duration>>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Clock {
    fn at(start: Duration) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
            events: Arc::default(),
        }
    }

    fn then(&self, at: Duration, event: impl FnOnce() + Send + 'static) {
        self.events.lock().unwrap().push((at, Box::new(event)));
    }

    fn now(&self) -> Duration {
        *self.now.lock().unwrap()
    }

    fn pace(&self) -> Pace {
        let now = self.now.clone();
        let advanced = self.now.clone();
        let events = self.events.clone();
        Pace {
            spacing: SPACING,
            elapsed: Box::new(move || *now.lock().unwrap()),
            wait: Box::new(move |wait| {
                let at = {
                    let mut now = advanced.lock().unwrap();
                    *now += wait;
                    *now
                };
                let due: Vec<_> = {
                    let mut events = events.lock().unwrap();
                    let (due, later) = events.drain(..).partition(|(when, _)| *when <= at);
                    *events = later;
                    due
                };
                due.into_iter().for_each(|(_, event)| event());
                true
            }),
        }
    }
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn turn(board: &Fake, pace: &mut Pace) -> String {
    take_turn(board, "work", pace).unwrap().unwrap()
}

#[test]
fn the_first_send_of_a_session_takes_its_turn_at_once() {
    let board = Fake::default();
    let clock = Clock::at(Duration::ZERO);
    turn(&board, &mut clock.pace());
    assert_eq!(clock.now(), Duration::ZERO);
}

#[test]
fn a_send_waits_the_spacing_after_the_attempt_before_it_ended() {
    let board = Arc::new(Fake::default());
    let clock = Clock::at(Duration::ZERO);
    let first = turn(&board, &mut clock.pace());
    let ending = board.clone();
    clock.then(ms(205), move || {
        end_turn(&*ending, "work", &first, true).unwrap();
    });
    *clock.now.lock().unwrap() = ms(10);
    turn(&board, &mut clock.pace());
    assert!(clock.now() >= ms(205) + SPACING, "{:?}", clock.now());
}

#[test]
fn one_client_waits_the_spacing_after_its_own_attempt() {
    let board = Fake::default();
    let clock = Clock::at(Duration::ZERO);
    let mut pace = clock.pace();
    let first = turn(&board, &mut pace);
    end_turn(&board, "work", &first, true).unwrap();
    let ended = clock.now();
    turn(&board, &mut pace);
    assert_eq!(clock.now() - ended, SPACING);
}

#[test]
fn two_clients_for_one_report_count_one_attempt() {
    let board = Arc::new(Fake::default());
    let clock = Clock::at(Duration::ZERO);
    let first = turn(&board, &mut clock.pace());
    let ending = board.clone();
    clock.then(ms(50), move || {
        end_turn(&*ending, "work", &first, true).unwrap();
    });
    let second = turn(&board, &mut clock.pace());
    end_turn(&*board, "work", &second, false).unwrap();
    assert_eq!(board.load().unwrap().0, json!({"work": {"sent": 1}}));
}

#[test]
fn a_dead_holder_blocks_the_session_only_until_the_hold_limit() {
    let board = Fake {
        document: Mutex::new((json!({"work": {"sent": 3, "holder": "gone"}}), 1)),
    };
    let clock = Clock::at(Duration::ZERO);
    let holder = turn(&board, &mut clock.pace());
    assert!(clock.now() >= HOLD_LIMIT, "{:?}", clock.now());
    assert!(clock.now() < HOLD_LIMIT + POLL, "{:?}", clock.now());
    assert_eq!(
        board.load().unwrap().0,
        json!({"work": {"sent": 3, "holder": holder}})
    );
}

#[test]
fn a_holder_whose_turn_was_taken_counts_its_send_and_keeps_the_new_holder() {
    let board = Fake {
        document: Mutex::new((json!({"work": {"sent": 3, "holder": "new"}}), 1)),
    };
    end_turn(&board, "work", "old", true).unwrap();
    assert_eq!(
        board.load().unwrap().0,
        json!({"work": {"sent": 4, "holder": "new"}})
    );
    end_turn(&board, "work", "old", false).unwrap();
    assert_eq!(board.load().unwrap().1, 2);
}

#[test]
fn other_sessions_and_an_entry_of_another_shape_are_kept_or_waited_out() {
    let board = Fake {
        document: Mutex::new((json!({"work": 1_000, "play": {"sent": 2}}), 1)),
    };
    let clock = Clock::at(Duration::ZERO);
    let holder = turn(&board, &mut clock.pace());
    assert_eq!(clock.now(), SPACING);
    assert_eq!(
        board.load().unwrap().0,
        json!({"work": {"sent": 0, "holder": holder}, "play": {"sent": 2}})
    );
}

#[test]
fn a_stop_during_the_wait_takes_no_turn() {
    let board = Fake {
        document: Mutex::new((json!({"work": {"sent": 1}}), 1)),
    };
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    drop(stop);
    let mut pace = Pace::new(Duration::from_secs(3600), stopped);
    assert_eq!(take_turn(&board, "work", &mut pace), Ok(None));
    assert_eq!(board.load().unwrap().1, 1);
}

#[test]
fn a_request_buffered_behind_a_wait_is_not_started_after_the_stop() {
    let (requests, queue) = std::sync::mpsc::channel();
    requests.send(()).unwrap();
    requests.send(()).unwrap();
    drop(requests);
    let (stop, stopped) = std::sync::mpsc::channel();
    let mut stop = Some(stop);
    let mut started = 0;
    drain(
        queue,
        Pace::new(Duration::from_secs(3600), stopped),
        |(), pace| {
            if (pace.wait)(Duration::ZERO) {
                started += 1;
                stop.take();
            }
        },
    );
    assert_eq!(started, 1);
}

fn stopping_after(mut pace: Pace, mut checks: usize) -> Pace {
    let mut wait = pace.wait;
    pace.wait = Box::new(move |duration| {
        if checks == 0 {
            return false;
        }
        checks -= 1;
        wait(duration)
    });
    pace
}

#[test]
fn a_stopped_client_takes_no_turn_even_without_a_wait() {
    let board = Fake::default();
    let clock = Clock::at(Duration::ZERO);
    let mut pace = stopping_after(clock.pace(), 0);
    assert_eq!(take_turn(&board, "work", &mut pace), Ok(None));
    assert_eq!(board.load().unwrap(), (Value::Null, 0));
}

#[test]
fn a_stop_after_the_turn_is_taken_sends_nothing_and_gives_the_turn_back_uncounted() {
    let board = Fake::default();
    let clock = Clock::at(Duration::ZERO);
    let mut pace = stopping_after(clock.pace(), 1);
    let mut sent = false;
    with_turn(&board, "work", &mut pace, || {
        sent = true;
        Ok(true)
    })
    .unwrap();
    assert!(!sent);
    assert_eq!(board.load().unwrap().0, json!({"work": {"sent": 0}}));
}

#[test]
fn a_dead_holder_is_taken_over_at_the_hold_limit_even_with_a_longer_spacing() {
    let board = Fake {
        document: Mutex::new((json!({"work": {"sent": 3, "holder": "gone"}}), 1)),
    };
    let clock = Clock::at(Duration::ZERO);
    let mut pace = clock.pace();
    pace.spacing = Duration::from_secs(60);
    turn(&board, &mut pace);
    assert!(clock.now() >= HOLD_LIMIT, "{:?}", clock.now());
    assert!(clock.now() < HOLD_LIMIT + POLL, "{:?}", clock.now());
}
