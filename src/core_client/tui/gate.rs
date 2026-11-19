use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::raw_input::RawInputEvent;

pub(super) const HOLD_LIMIT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct Gate {
    read: VecDeque<RawInputEvent>,
    closed: Option<Instant>,
    editing: usize,
    abandoned: usize,
    written: Option<u64>,
    arriving: Option<u64>,
    shown: Option<u64>,
}

impl Gate {
    pub(super) async fn next(
        &mut self,
        input: &mut mpsc::UnboundedReceiver<Vec<RawInputEvent>>,
    ) -> Option<Vec<RawInputEvent>> {
        loop {
            if let Some(batch) = self.take(Instant::now()) {
                return Some(batch);
            }
            self.read.extend(input.recv().await?);
        }
    }

    pub(super) fn unread(&mut self, rest: impl IntoIterator<Item = RawInputEvent>) {
        let mut rest: VecDeque<_> = rest.into_iter().collect();
        rest.append(&mut self.read);
        self.read = rest;
    }

    pub(super) fn queued(&mut self, edits: usize, now: Instant) {
        if edits > 0 {
            self.editing += edits;
            self.closed.get_or_insert(now);
        }
    }

    pub(super) fn finished(&mut self, written: Option<u64>) {
        if self.abandoned > 0 {
            self.abandoned -= 1;
            return;
        }
        self.editing = self.editing.saturating_sub(1);
        self.written = self.written.max(written);
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
        Some(self.closed? + HOLD_LIMIT)
    }

    fn take(&mut self, now: Instant) -> Option<Vec<RawInputEvent>> {
        if let Some(closed) = self.closed {
            let caught_up = self.editing == 0
                && self
                    .written
                    .is_none_or(|written| self.shown.is_some_and(|shown| shown >= written));
            if !caught_up && now.duration_since(closed) < HOLD_LIMIT {
                return None;
            }
            self.abandoned += std::mem::take(&mut self.editing);
            self.written = None;
            self.closed = None;
        }
        (!self.read.is_empty()).then(|| self.read.drain(..).collect())
    }
}

#[cfg(test)]
mod tests;
