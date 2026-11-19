use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct Opener {
    pub(super) terminal: String,
    pub(super) zoomed: bool,
}

pub(super) struct Settled<T> {
    pub(super) shown: Option<(T, bool)>,
    pub(super) gone: Vec<(String, Opener)>,
}

struct Start {
    opener: Option<Opener>,
    placed: Option<(String, u64)>,
    owned: bool,
}

struct Waiting {
    request: u64,
    terminal: String,
    returns: bool,
}

#[derive(Default)]
pub(super) struct Starts {
    asked: u64,
    quiet_through: u64,
    pending: HashMap<u64, Start>,
    waiting: Option<Waiting>,
    version: u64,
}

impl<T> Default for Settled<T> {
    fn default() -> Self {
        Self {
            shown: None,
            gone: Vec::new(),
        }
    }
}

impl<T> Settled<T> {
    pub(super) fn is_empty(&self) -> bool {
        self.shown.is_none() && self.gone.is_empty()
    }
}

impl Starts {
    pub(super) fn ask(&mut self, opener: Option<Opener>) -> u64 {
        self.asked += 1;
        self.pending.insert(
            self.asked,
            Start {
                opener,
                placed: None,
                owned: false,
            },
        );
        self.asked
    }

    pub(super) fn started<T>(
        &mut self,
        request: u64,
        placed: Option<(String, u64)>,
        owns: impl Fn(&str) -> bool,
        locate: impl FnOnce(&str) -> Option<T>,
    ) -> Settled<T> {
        let Some(mut start) = self.pending.remove(&request) else {
            return Settled::default();
        };
        if let Some((terminal, _)) = &placed {
            self.waiting = (request > self.quiet_through).then(|| Waiting {
                request,
                terminal: terminal.clone(),
                returns: start.opener.is_some(),
            });
            if start.opener.is_some() {
                start.placed = placed;
                self.pending.insert(request, start);
            }
        }
        self.settle(owns, locate)
    }

    pub(super) fn laid_out<T>(
        &mut self,
        changed: bool,
        owns: impl Fn(&str) -> bool,
        locate: impl FnOnce(&str) -> Option<T>,
        version: u64,
    ) -> Settled<T> {
        self.version = version;
        if changed {
            for start in self.pending.values_mut() {
                start.owned = false;
            }
        }
        self.settle(owns, locate)
    }

    pub(super) fn moved(&mut self) {
        self.quiet_through = self.asked;
        self.waiting = None;
        self.pending.retain(|_, start| start.opener.is_some());
    }

    fn settle<T>(
        &mut self,
        owns: impl Fn(&str) -> bool,
        locate: impl FnOnce(&str) -> Option<T>,
    ) -> Settled<T> {
        let mut gone = Vec::new();
        for (request, start) in &mut self.pending {
            let Some((terminal, version)) = &start.placed else {
                continue;
            };
            if start.owned || self.version < *version {
                continue;
            }
            if owns(terminal) {
                start.owned = true;
            } else {
                gone.push(*request);
            }
        }
        let mut settled = Settled::default();
        for request in gone {
            let Some(Start {
                opener: Some(opener),
                placed: Some((terminal, _)),
                ..
            }) = self.pending.remove(&request)
            else {
                continue;
            };
            if self
                .waiting
                .as_ref()
                .is_some_and(|waiting| waiting.request == request)
            {
                self.waiting = None;
            }
            let links = self
                .pending
                .values_mut()
                .filter_map(|start| start.opener.as_mut())
                .chain(settled.gone.iter_mut().map(|(_, opener)| opener));
            for link in links {
                if link.terminal == terminal {
                    link.clone_from(&opener);
                }
            }
            settled.gone.push((terminal, opener));
        }
        if let Some(waiting) = &self.waiting {
            if let Some(focus) = locate(&waiting.terminal) {
                settled.shown = Some((focus, waiting.returns));
                self.waiting = None;
            }
        }
        settled
    }

    #[cfg(test)]
    pub(super) fn version(&self) -> u64 {
        self.version
    }

    #[cfg(test)]
    pub(super) fn tracked(&self) -> Vec<(u64, Option<&str>)> {
        let mut tracked: Vec<_> = self
            .pending
            .iter()
            .map(|(request, start)| {
                (
                    *request,
                    start.placed.as_ref().map(|(terminal, _)| terminal.as_str()),
                )
            })
            .chain(
                self.waiting
                    .as_ref()
                    .filter(|waiting| !waiting.returns)
                    .map(|waiting| (waiting.request, Some(waiting.terminal.as_str()))),
            )
            .collect();
        tracked.sort_unstable();
        tracked
    }
}
