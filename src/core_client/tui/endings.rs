use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq)]
pub(super) enum Ending {
    Snapshot {
        finished: BTreeSet<String>,
        live: BTreeSet<String>,
    },
    Finished(String),
    Live(String),
    Removed(String),
}

#[derive(Default)]
pub(super) struct Endings {
    finished: BTreeSet<String>,
    owned: BTreeSet<String>,
    due: BTreeMap<String, bool>,
}

impl Endings {
    pub(super) fn own<'a>(&mut self, terminals: impl IntoIterator<Item = &'a str>) {
        self.owned = terminals.into_iter().map(str::to_owned).collect();
        let owned = &self.owned;
        self.due.retain(|terminal, _| owned.contains(terminal));
        self.add_owned_finished();
    }

    pub(super) fn hear(&mut self, ending: Ending) {
        match ending {
            Ending::Snapshot { finished, live } => {
                self.due.retain(|terminal, _| !live.contains(terminal));
                self.finished = finished;
                self.add_owned_finished();
            }
            Ending::Finished(terminal) => {
                if self.owned.contains(&terminal) {
                    self.due.entry(terminal.clone()).or_insert(false);
                }
                self.finished.insert(terminal);
            }
            Ending::Live(terminal) => {
                self.finished.remove(&terminal);
                self.due.remove(&terminal);
            }
            Ending::Removed(terminal) => {
                self.finished.remove(&terminal);
            }
        }
    }

    pub(super) fn closing(&mut self, viewed: impl Fn(&str) -> bool) -> Vec<String> {
        self.due
            .iter_mut()
            .filter(|(terminal, asked)| !**asked && !viewed(terminal))
            .map(|(terminal, asked)| {
                *asked = true;
                terminal.clone()
            })
            .collect()
    }

    pub(super) fn ask(&mut self, terminal: &str) -> bool {
        if !self.owned.contains(terminal) {
            return true;
        }
        !std::mem::replace(self.due.entry(terminal.to_owned()).or_default(), true)
    }

    fn add_owned_finished(&mut self) {
        for terminal in self.owned.intersection(&self.finished) {
            self.due.entry(terminal.clone()).or_insert(false);
        }
    }
}

#[cfg(test)]
mod tests;
