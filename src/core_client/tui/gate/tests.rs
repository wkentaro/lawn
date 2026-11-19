use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use tokio::sync::mpsc;

use super::{Gate, HOLD_LIMIT};
use crate::input::TerminalKey;
use crate::raw_input::RawInputEvent;

fn polled<T>(future: impl Future<Output = T>) -> Option<T> {
    match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

fn key(c: char) -> RawInputEvent {
    RawInputEvent::Key(TerminalKey::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn press() -> RawInputEvent {
    RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 4,
        modifiers: KeyModifiers::NONE,
    })
}

fn said(events: impl IntoIterator<Item = RawInputEvent>) -> Vec<String> {
    events
        .into_iter()
        .map(|event| format!("{event:?}"))
        .collect()
}

fn drained(gate: &mut Gate, now: Instant) -> Vec<String> {
    said(std::iter::from_fn(|| gate.take(now)).collect::<Vec<_>>())
}

fn read(gate: &mut Gate, events: impl IntoIterator<Item = RawInputEvent>) {
    gate.read.extend(events);
}

fn shown(gate: &mut Gate, version: u64) {
    gate.arrived(version, true);
    gate.drawn();
}

fn editing(gate: &mut Gate, now: Instant) {
    read(gate, [key('s')]);
    assert_eq!(drained(gate, now), said([key('s')]));
    gate.queued(1, now);
}

#[test]
fn input_comes_straight_out_while_no_edit_is_in_flight() {
    let mut gate = Gate::default();
    let now = Instant::now();

    read(&mut gate, [key('a'), key('b')]);

    assert_eq!(drained(&mut gate, now), said([key('a'), key('b')]));
    assert_eq!(gate.deadline(), None);
}

#[test]
fn a_key_read_after_a_swap_waits_for_the_edit_and_a_layout_with_its_version() {
    let mut gate = Gate::default();
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    editing(&mut gate, at(0));

    read(&mut gate, [key('h')]);
    assert!(drained(&mut gate, at(10)).is_empty());
    gate.finished(Some(7));
    assert!(drained(&mut gate, at(20)).is_empty());
    shown(&mut gate, 6);
    assert!(drained(&mut gate, at(30)).is_empty());
    shown(&mut gate, 7);

    assert_eq!(drained(&mut gate, at(40)), said([key('h')]));
}

#[test]
fn an_edit_that_wrote_nothing_or_failed_opens_the_gate_when_it_finishes() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    assert!(drained(&mut gate, now).is_empty());

    gate.finished(None);

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn two_edits_in_flight_open_the_gate_only_after_both_and_at_the_higher_version() {
    let mut gate = Gate::default();
    let now = Instant::now();
    read(&mut gate, [key('s')]);
    drained(&mut gate, now);
    gate.queued(2, now);
    read(&mut gate, [key('h')]);

    gate.finished(Some(5));
    shown(&mut gate, 5);
    assert!(drained(&mut gate, now).is_empty());
    gate.finished(Some(6));
    assert!(drained(&mut gate, now).is_empty());
    shown(&mut gate, 6);

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn a_layout_older_than_the_finished_edit_keeps_the_gate_closed() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    shown(&mut gate, 7);

    gate.finished(Some(8));
    shown(&mut gate, 7);

    assert!(drained(&mut gate, now).is_empty());
}

#[test]
fn a_layout_that_arrives_before_its_edit_finishes_opens_the_gate_at_the_finish() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    shown(&mut gate, 8);
    assert!(drained(&mut gate, now).is_empty());

    gate.finished(Some(8));

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn the_hold_limit_opens_the_gate_and_input_read_meanwhile_comes_out_in_order() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    read(&mut gate, [key('a')]);
    read(&mut gate, [key('b'), key('c')]);

    assert_eq!(gate.deadline(), Some(start + HOLD_LIMIT));
    assert!(drained(&mut gate, start + HOLD_LIMIT - Duration::from_millis(1)).is_empty());

    assert_eq!(
        drained(&mut gate, start + HOLD_LIMIT),
        said([key('a'), key('b'), key('c')])
    );
    assert_eq!(gate.deadline(), None);
}

#[test]
fn edits_queued_while_a_timed_out_edit_is_unanswered_hold_nothing_until_every_late_finish_is_in() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    drained(&mut gate, start + HOLD_LIMIT);
    let later = start + HOLD_LIMIT * 2;
    editing(&mut gate, later);
    gate.finished(None);
    read(&mut gate, [key('h')]);
    assert_eq!(drained(&mut gate, later), said([key('h')]));
    gate.finished(None);

    editing(&mut gate, later);
    read(&mut gate, [key('j')]);
    assert!(drained(&mut gate, later).is_empty());
    gate.finished(None);

    assert_eq!(drained(&mut gate, later), said([key('j')]));
}

fn escape_after_resize_repeats(answer: impl Fn(&mut Gate, u64)) -> Duration {
    let mut gate = Gate::default();
    let start = Instant::now();
    let escape = || RawInputEvent::Key(TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));
    read(&mut gate, (0..5).map(|_| key('l')).chain([escape()]));
    let mut now = start;
    let mut version = 0;

    loop {
        let Some(event) = gate.take(now) else {
            now = gate.deadline().expect("a hold");
            continue;
        };
        if said([event]) == said([escape()]) {
            break;
        }
        gate.queued(1, now);
        version += 1;
        answer(&mut gate, version);
    }

    now - start
}

#[test]
fn escape_after_a_burst_of_resize_repeats_waits_one_hold_limit_when_the_core_does_not_answer() {
    assert_eq!(escape_after_resize_repeats(|_, _| {}), HOLD_LIMIT);
}

#[test]
fn escape_after_a_burst_of_resize_repeats_waits_one_hold_limit_when_each_write_is_answered_but_no_layout_arrives(
) {
    assert_eq!(
        escape_after_resize_repeats(|gate, version| gate.finished(Some(version))),
        HOLD_LIMIT
    );
}

#[test]
fn holds_start_again_once_the_layout_at_the_last_written_version_is_shown() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    gate.finished(Some(9));
    drained(&mut gate, start + HOLD_LIMIT);
    let later = start + HOLD_LIMIT * 2;
    editing(&mut gate, later);
    gate.finished(Some(10));
    shown(&mut gate, 9);

    editing(&mut gate, later);
    read(&mut gate, [key('h')]);
    assert_eq!(drained(&mut gate, later), said([key('h')]));
    gate.finished(None);
    shown(&mut gate, 10);
    editing(&mut gate, later);
    read(&mut gate, [key('j')]);

    assert!(drained(&mut gate, later).is_empty());
}

#[test]
fn after_a_lapsed_hold_versions_that_restart_lower_still_let_holds_start_again() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    gate.finished(Some(100));
    drained(&mut gate, start + HOLD_LIMIT);
    let later = start + HOLD_LIMIT * 2;
    editing(&mut gate, later);
    gate.finished(Some(4));
    shown(&mut gate, 4);

    editing(&mut gate, later);
    read(&mut gate, [key('h')]);

    assert!(drained(&mut gate, later).is_empty());
}

#[test]
fn the_rest_of_a_batch_after_the_event_that_queued_an_edit_comes_out_first_once_open() {
    let mut gate = Gate::default();
    let now = Instant::now();
    read(&mut gate, [key('s'), key('x'), key('y')]);
    assert_eq!(said(gate.take(now)), said([key('s')]));

    gate.queued(1, now);
    read(&mut gate, [key('z')]);
    assert!(drained(&mut gate, now).is_empty());
    gate.finished(None);

    assert_eq!(
        drained(&mut gate, now),
        said([key('x'), key('y'), key('z')])
    );
}

#[test]
fn mouse_and_paste_are_held_like_keys_and_keep_their_order() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    let input = || [press(), RawInputEvent::Paste("p".to_owned()), key('k')];

    read(&mut gate, input());
    assert!(drained(&mut gate, now).is_empty());
    gate.finished(None);

    assert_eq!(drained(&mut gate, now), said(input()));
}

#[tokio::test]
async fn host_focus_is_held_with_the_rest_and_keeps_the_order_it_was_read_in() {
    let mut gate = Gate::default();
    let now = Instant::now();
    let (host, mut input) = mpsc::unbounded_channel();
    host.send(vec![key('s'), RawInputEvent::OuterFocusLost])
        .expect("sent");
    assert_eq!(gate.ready(&mut input).await, Some(()));
    assert_eq!(said(gate.take(now)), said([key('s')]));
    gate.queued(1, now);
    host.send(vec![RawInputEvent::OuterFocusGained])
        .expect("sent");
    drop(host);
    assert_eq!(gate.ready(&mut input).await, None);

    gate.finished(None);

    assert_eq!(
        drained(&mut gate, now),
        said([
            RawInputEvent::OuterFocusLost,
            RawInputEvent::OuterFocusGained
        ])
    );
}

#[test]
fn after_versions_restart_lower_an_edit_still_waits_for_a_layout_with_its_version() {
    let mut gate = Gate::default();
    let now = Instant::now();
    shown(&mut gate, 100);
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    shown(&mut gate, 3);

    gate.finished(Some(4));
    assert!(drained(&mut gate, now).is_empty());
    shown(&mut gate, 4);

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn a_layout_counts_for_the_hold_only_once_the_frame_that_lays_it_out_is_drawn() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    gate.finished(Some(7));

    gate.arrived(7, true);
    assert!(drained(&mut gate, now).is_empty());
    gate.drawn();

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn an_unchanged_layout_counts_at_once_when_no_frame_is_pending() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    gate.finished(Some(7));

    gate.arrived(7, false);

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn an_unchanged_layout_behind_a_pending_frame_waits_for_that_frame() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);
    gate.arrived(6, true);
    gate.finished(Some(7));

    gate.arrived(7, false);
    assert!(drained(&mut gate, now).is_empty());
    gate.drawn();

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn an_edit_queued_after_the_client_catches_up_from_a_lapsed_hold_holds_before_input_is_read() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    gate.finished(Some(9));
    drained(&mut gate, start + HOLD_LIMIT);
    shown(&mut gate, 9);
    let later = start + HOLD_LIMIT * 2;

    gate.queued(1, later);
    read(&mut gate, [key('h')]);

    assert!(drained(&mut gate, later).is_empty());
}

#[test]
fn after_versions_restart_lower_an_edit_that_writes_nothing_opens_the_gate_at_its_finish() {
    let mut gate = Gate::default();
    let now = Instant::now();
    editing(&mut gate, now);
    gate.finished(Some(100));
    shown(&mut gate, 100);
    drained(&mut gate, now);
    shown(&mut gate, 3);
    editing(&mut gate, now);
    read(&mut gate, [key('h')]);

    gate.finished(None);

    assert_eq!(drained(&mut gate, now), said([key('h')]));
}

#[test]
fn handling_an_edit_in_a_backlog_while_lapsed_leaves_the_rest_of_the_backlog_where_it_is() {
    let mut gate = Gate::default();
    let start = Instant::now();
    editing(&mut gate, start);
    drained(&mut gate, start + HOLD_LIMIT);
    let later = start + HOLD_LIMIT * 2;
    read(&mut gate, (0..100).map(|_| key('l')));
    let last = gate.read.back().map(std::ptr::from_ref);

    gate.take(later);
    gate.queued(1, later);

    assert_eq!(gate.read.len(), 99);
    assert_eq!(gate.read.back().map(std::ptr::from_ref), last);
}

#[test]
fn input_held_behind_an_edit_becomes_ready_when_the_edit_finishes_without_new_input() {
    let mut gate = Gate::default();
    let far = Instant::now() + Duration::from_secs(3600);
    let (_host, mut input) = mpsc::unbounded_channel();
    editing(&mut gate, far);
    read(&mut gate, [key('h')]);
    assert_eq!(polled(gate.ready(&mut input)), None);

    gate.finished(None);

    assert_eq!(polled(gate.ready(&mut input)), Some(Some(())));
}
