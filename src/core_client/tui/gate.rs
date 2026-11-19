use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::raw_input::RawInputEvent;

pub(super) const HOLD_LIMIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Default, PartialEq)]
enum Hold {
    #[default]
    Open,
    Closed(Instant),
    Lapsed,
}

#[derive(Default)]
pub(super) struct Gate {
    read: VecDeque<RawInputEvent>,
    hold: Hold,
    editing: usize,
    written: Option<u64>,
    arriving: Option<u64>,
    shown: Option<u64>,
}

impl Gate {
    pub(super) async fn ready(
        &mut self,
        input: &mut mpsc::UnboundedReceiver<Vec<RawInputEvent>>,
    ) -> Option<()> {
        loop {
            self.settle(Instant::now());
            if !self.holding() && !self.read.is_empty() {
                return Some(());
            }
            self.read.extend(input.recv().await?);
        }
    }

    pub(super) fn queued(&mut self, edits: usize, now: Instant) {
        self.settle(now);
        self.editing += edits;
        if edits > 0 && self.hold == Hold::Open {
            self.hold = Hold::Closed(now);
        }
    }

    pub(super) fn finished(&mut self, written: Option<u64>) {
        self.editing = self.editing.saturating_sub(1);
        self.written = written.or(self.written);
    }

    pub(super) fn arrived(&mut self, version: u64, redraws: bool) {
        if redraws || self.arriving.is_some() {
            self.arriving = Some(version);
        } else {
            self.shown = Some(version);
        }
    }

    pub(super) fn drawn(&mut self) {
        if let Some(version) = self.arriving.take() {
            self.shown = Some(version);
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        match self.hold {
            Hold::Closed(since) => Some(since + HOLD_LIMIT),
            Hold::Open | Hold::Lapsed => None,
        }
    }

    fn caught_up(&self) -> bool {
        self.editing == 0
            && self
                .written
                .is_none_or(|written| self.shown.is_some_and(|shown| shown >= written))
    }

    fn settle(&mut self, now: Instant) {
        if self.caught_up() {
            self.hold = Hold::Open;
            self.written = None;
        } else if let Hold::Closed(since) = self.hold {
            if now.duration_since(since) >= HOLD_LIMIT {
                self.hold = Hold::Lapsed;
            }
        }
    }

    pub(super) fn take(&mut self, now: Instant) -> Option<RawInputEvent> {
        self.settle(now);
        if self.holding() {
            return None;
        }
        self.read.pop_front()
    }

    pub(super) fn holding(&self) -> bool {
        matches!(self.hold, Hold::Closed(_))
    }
}

#[cfg(test)]
mod tests;
