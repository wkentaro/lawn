use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Starts {
    asked: u64,
    unanswered: HashSet<u64>,
    waiting: Option<(u64, String)>,
}

impl Starts {
    pub(super) fn ask(&mut self) -> u64 {
        self.asked += 1;
        self.unanswered.insert(self.asked);
        self.asked
    }

    pub(super) fn started<T>(
        &mut self,
        request: u64,
        terminal: Option<String>,
        locate: impl FnOnce(&str) -> Option<T>,
    ) -> Option<T> {
        if !self.unanswered.remove(&request) {
            return None;
        }
        let terminal = terminal?;
        let located = locate(&terminal);
        self.waiting = located.is_none().then_some((request, terminal));
        located
    }

    pub(super) fn laid_out<T>(&mut self, locate: impl FnOnce(&str) -> Option<T>) -> Option<T> {
        let located = locate(&self.waiting.as_ref()?.1)?;
        self.waiting = None;
        Some(located)
    }

    #[cfg(test)]
    pub(super) fn tracked(&self) -> Vec<(u64, Option<&str>)> {
        let mut tracked: Vec<_> = self
            .unanswered
            .iter()
            .map(|request| (*request, None))
            .chain(
                self.waiting
                    .as_ref()
                    .map(|(request, terminal)| (*request, Some(terminal.as_str()))),
            )
            .collect();
        tracked.sort_unstable();
        tracked
    }
}
