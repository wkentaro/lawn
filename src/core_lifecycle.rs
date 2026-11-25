use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use crate::hc::{Core, HcError};
use crate::session_doc::{Created, Destination, Layout, Sessions, Store};

const START_LABEL: &str = "lawn-start";
const KEEP_SETTLE_ATTEMPTS: usize = 4;

fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
}

pub(crate) struct Lifecycle {
    core: Arc<Core>,
    session: String,
    on: Option<String>,
    store: Store,
}

impl Lifecycle {
    pub(crate) fn new(core: Arc<Core>, session: &str) -> Self {
        Self::on(core, session, None)
    }

    pub(crate) fn on(core: Arc<Core>, session: &str, machine: Option<&str>) -> Self {
        let on = machine.map(str::to_owned);
        Self {
            store: Store::new(core.clone(), session, on.clone()),
            core,
            session: session.to_owned(),
            on,
        }
    }

    fn address(&self, terminal: &str) -> String {
        crate::hc::qualified(self.on.as_deref(), terminal)
    }

    pub(crate) fn layout(&self) -> Result<Layout, String> {
        Ok(self.store.load()?.layout)
    }

    /// Records the start, then submits terminal.run exactly once. Failure
    /// leaves the record for recovery and never automatically repeats run.
    pub(crate) fn start(
        &self,
        destination: Destination,
        cwd: Option<&Path>,
        argv: &[String],
    ) -> Result<Created, String> {
        let cwd = cwd.map(utf8).transpose()?;
        let reference = self.store.reserve_start(&destination)?;
        self.submit(&reference, cwd, argv)?.ok_or_else(|| {
            format!(
                "start {reference} was already recovered; inspect `lawn --session {} ls`",
                self.session
            )
        })
    }

    #[cfg(unix)]
    pub(crate) fn open(&self, cwd: &Path) -> Result<(), String> {
        let settled = self.store.load()?.layout.pending_starts.is_empty();
        if let Err(error) = self.recover() {
            eprintln!("warning: recovery in session {}: {error}", self.session);
        }
        if settled {
            self.start_first(cwd, &[])?;
        }
        Ok(())
    }

    pub(crate) fn start_first(
        &self,
        cwd: &Path,
        argv: &[String],
    ) -> Result<Option<Created>, String> {
        let cwd = utf8(cwd)?;
        match self.store.reserve_first_start()? {
            Some(reference) => self.submit(&reference, Some(cwd), argv),
            None => Ok(None),
        }
    }

    fn submit(
        &self,
        reference: &str,
        cwd: Option<&str>,
        argv: &[String],
    ) -> Result<Option<Created>, String> {
        let mut params = json!({
            "on": self.on,
            "cwd": cwd,
            "keep": true,
            "no_parent": true,
            "no_notify": true,
            "name": start_name(reference),
            "labels": {START_LABEL: reference},
        });
        if !argv.is_empty() {
            params["argv"] = json!(argv);
        }
        let terminal = match self.core.run(params) {
            Ok(terminal) => terminal,
            Err(err @ HcError::Rpc { .. }) => {
                if is_final(self.on.as_deref(), err.kind()) {
                    self.store.edit(|layout| {
                        if let Some(start) = layout.pending_starts.get_mut(reference) {
                            start.reply_received = true;
                        }
                    }).map_err(|recovery| format!("{err}; start {reference} remains unconfirmed: {recovery}"))?;
                }
                self.recover().map_err(|recovery| format!("{err}; recovery of {reference}: {recovery}"))?;
                return Err(err.to_string());
            }
            Err(err) => return Err(format!(
                "start {reference} is unconfirmed: {err}; inspect `lawn --session {} ls` before starting another program", self.session
            )),
        };
        self.finish_start(reference, local_of(&terminal))
    }

    /// Recovers saved starts and closes without ever starting another
    /// program. An empty lookup without a final reply leaves it unresolved.
    pub(crate) fn recover(&self) -> Result<(), String> {
        let loaded = self.store.load()?.layout;
        for (reference, start) in &loaded.pending_starts {
            if let Some(terminal) = self.lookup(reference)? {
                self.finish_start(reference, &terminal)?;
            } else if start.reply_received {
                self.store
                    .edit(|layout| layout.pending_starts.remove(reference))?;
            }
        }
        for terminal in self.store.load()?.layout.closing {
            self.end_close(&terminal)?;
        }
        Ok(())
    }

    fn lookup(&self, reference: &str) -> Result<Option<String>, String> {
        let list = |filter: serde_json::Value| {
            self.core
                .call("terminal.list", filter)
                .map_err(|err| err.to_string())
        };
        if self.on.is_none() {
            let listed = list(json!({"all": true, "labels": {START_LABEL: reference}}))?;
            return Ok(started(&listed, None, reference)?.map(str::to_owned));
        }
        let named = self.address(&start_name(reference));
        let record = match self.core.call("terminal.get", json!({"terminal": named})) {
            Ok(record) => record,
            Err(err) if err.kind() == Some("not_found") => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        let holder = record["address"]
            .as_str()
            .ok_or("the core returned no terminal address")?;
        let (machine, local) = crate::hc::located(holder)?;
        if record["labels"][START_LABEL] == reference {
            return Ok(Some(local.to_owned()));
        }
        let listed = list(json!({"all": true}))?;
        if !confirms(&listed, self.on.as_deref(), machine, holder) {
            return Err(format!(
                "the terminal list does not show {holder} on the same machine, so start {reference} cannot be confirmed; run the command again"
            ));
        }
        Ok(started(&listed, machine, reference)?.map(str::to_owned))
    }

    pub(crate) fn drop_start(&self, reference: &str) -> Result<(), String> {
        let gone = || {
            format!(
                "start {reference} is not pending in session {}",
                self.session
            )
        };
        let pending = self.store.load()?.layout.pending_starts;
        if !pending.contains_key(reference) {
            return Err(gone());
        }
        if let Some(terminal) = self.lookup(reference)? {
            self.finish_start(reference, &terminal)?;
            return Err(gone());
        }
        let (dropped, _) = self
            .store
            .edit(|layout| layout.pending_starts.remove(reference).is_some())?;
        dropped.then_some(()).ok_or_else(gone)
    }

    fn finish_start(&self, reference: &str, terminal: &str) -> Result<Option<Created>, String> {
        let (finished, _) = self.store.edit_all(|sessions| {
            let Some(start) = sessions.get(&self.session).and_then(|l| l.pending_starts.get(reference)).cloned() else {
                return Ok(None);
            };
            if let Some(owner) = owner(sessions, terminal) {
                return Err(format!("{terminal} is already owned by session {owner}; start {reference} remains pending"));
            }
            let Some(layout) = sessions.get_mut(&self.session) else {
                return Ok(None);
            };
            match start.destination.place(layout, terminal) {
                Ok(created) => {
                    layout.pending_starts.remove(reference);
                    Ok(Some(created))
                }
                Err(err) => {
                    layout.pending_starts.remove(reference);
                    layout.closing.insert(terminal.to_owned());
                    Err(format!("{err}; {terminal} is reserved for closing"))
                }
            }
        })?;
        finished
    }

    pub(crate) fn close(&self, terminal: &str) -> Result<(), String> {
        let (reserved, _) = self.store.edit(|layout| {
            let ours = layout.remove(terminal) | layout.closing.contains(terminal);
            if ours {
                layout.closing.insert(terminal.to_owned());
            }
            ours
        })?;
        if !reserved {
            return Err(format!("{terminal} is not in session {}", self.session));
        }
        self.end_close(terminal)
    }

    fn end_close(&self, terminal: &str) -> Result<(), String> {
        match self
            .core
            .call("terminal.kill", json!({"terminal": self.address(terminal)}))
        {
            Ok(record) if record["state"] == "live" => {
                return Err(format!("{terminal} has not ended; close remains pending"))
            }
            Ok(_) => {}
            Err(err) => self.ended_or(terminal, err.to_string())?,
        }
        self.store.edit(|layout| layout.closing.remove(terminal))?;
        Ok(())
    }

    pub(crate) fn park(&self, terminal: &str) -> Result<(), String> {
        let (parked, _) = self
            .store
            .edit(|layout| layout.park(terminal) || layout.parked.contains(terminal))?;
        if !parked {
            return Err(format!(
                "{terminal} is not shown in session {}",
                self.session
            ));
        }
        self.settle_keep(terminal)
    }

    #[cfg(unix)]
    pub(crate) fn arrange(
        &self,
        machine: Option<&str>,
        arrangement: &crate::session_doc::Arrangement,
    ) -> Result<Option<u64>, String> {
        let (found, loaded, wrote) = self.store.edited(|layout| match machine {
            None => layout.arrange(arrangement),
            Some(machine) => {
                let mut listed = std::mem::take(layout).qualified(machine);
                let found = listed.arrange(arrangement);
                *layout = listed.readdressed(|far| local_of(far).to_owned());
                found
            }
        })?;
        if !found {
            return Err(format!(
                "session {} has nothing for {arrangement:?}",
                self.session
            ));
        }
        Ok(wrote.then_some(loaded.version))
    }

    pub(crate) fn unpark(
        &self,
        terminal: &str,
        to: Option<crate::session_doc::Placement>,
    ) -> Result<String, String> {
        let record = self
            .core
            .call("terminal.get", json!({"terminal": self.address(terminal)}))
            .map_err(|err| err.to_string())?;
        let terminal = local_of(
            record["address"]
                .as_str()
                .ok_or("the core returned no terminal address")?,
        );
        if record["labels"].get("lawn").is_some() {
            return Err(format!(
                "{terminal} belongs to a Lawn session outside the rewrite"
            ));
        }
        let reference = record["labels"][START_LABEL].as_str();
        let (claimed, _) = self.store.edit_all(|sessions| {
            if reference.is_some_and(|reference| {
                sessions
                    .values()
                    .any(|l| l.pending_starts.contains_key(reference))
            }) {
                return Err(format!(
                    "{terminal} has an unconfirmed start; recover its session first"
                ));
            }
            claim(sessions, &self.session, terminal, to.clone())
        })?;
        claimed?;
        self.settle_keep(terminal)?;
        Ok(terminal.to_owned())
    }

    /// Reconciles keep against the current layout. Concurrent core calls are
    /// not fenced by document CAS; a caller must retry a reported failure.
    pub(crate) fn settle_keep(&self, terminal: &str) -> Result<(), String> {
        let shown_anywhere = || -> Result<bool, String> {
            let (sessions, _) = self.store.load_all()?;
            Ok(sessions.values().any(|layout| layout.shows(terminal)))
        };
        let mut shown = shown_anywhere()?;
        for _ in 0..KEEP_SETTLE_ATTEMPTS {
            if let Err(err) = self.core.call(
                "terminal.keep",
                json!({"terminal": self.address(terminal), "keep": shown}),
            ) {
                self.ended_or(terminal, err.to_string())?;
            }
            let now = shown_anywhere()?;
            if now == shown {
                return Ok(());
            }
            shown = now;
        }
        Err(format!("{terminal} kept moving; run the command again"))
    }

    fn ended_or(&self, terminal: &str, err: String) -> Result<(), String> {
        match self
            .core
            .call("terminal.get", json!({"terminal": self.address(terminal)}))
        {
            Ok(record) if record["state"] != "live" => Ok(()),
            Err(missing) if missing.kind() == Some("not_found") => Ok(()),
            _ => Err(err),
        }
    }
}

fn is_final(on: Option<&str>, kind: Option<&str>) -> bool {
    on.is_none() || kind != Some("unavailable")
}

fn start_name(reference: &str) -> String {
    reference.replace(':', "-")
}

fn local_of(address: &str) -> &str {
    address.rsplit_once('/').map_or(address, |(_, local)| local)
}

fn confirms(
    listed: &serde_json::Value,
    on: Option<&str>,
    machine: Option<&str>,
    holder: &str,
) -> bool {
    let list = |key: &str| listed[key].as_array().into_iter().flatten();
    let named = |key: &str| list("machines").find(|machine| on == machine[key].as_str());
    let alias = named("id").or_else(|| named("name"));
    machine == alias.and_then(|machine| machine["name"].as_str())
        && list("terminals").any(|terminal| terminal["address"] == holder)
}

fn started<'a>(
    listed: &'a serde_json::Value,
    machine: Option<&str>,
    reference: &str,
) -> Result<Option<&'a str>, String> {
    let terminals = listed["terminals"]
        .as_array()
        .ok_or("the core returned no terminal list")?;
    let mut found = None;
    for terminal in terminals {
        let address = terminal["address"]
            .as_str()
            .ok_or("the core returned no terminal address")?;
        let local = match crate::hc::located(address) {
            Ok((on, local)) if on == machine && terminal["labels"][START_LABEL] == reference => {
                local
            }
            _ => continue,
        };
        if found.replace(local).is_some() {
            return Err(format!(
                "the start identifies multiple {} terminals; nothing was changed",
                machine.unwrap_or("local")
            ));
        }
    }
    Ok(found)
}

fn owner<'a>(sessions: &'a Sessions, terminal: &str) -> Option<&'a str> {
    sessions
        .iter()
        .find(|(_, layout)| layout.owns(terminal) || layout.closing.contains(terminal))
        .map(|(session, _)| session.as_str())
}

fn claim(
    sessions: &mut Sessions,
    session: &str,
    terminal: &str,
    to: Option<crate::session_doc::Placement>,
) -> Result<(), String> {
    if sessions.values().any(|l| l.closing.contains(terminal)) {
        return Err(format!("{terminal} is closing"));
    }
    if let Some((owner, _)) = sessions
        .iter()
        .find(|(name, l)| name.as_str() != session && l.shows(terminal))
    {
        return Err(format!("{terminal} is shown in session {owner}"));
    }
    if sessions.get(session).is_some_and(|l| l.shows(terminal)) {
        return Ok(());
    }
    if let Some(layout) = sessions.get_mut(session) {
        layout.unpark(terminal, to)?;
    } else {
        let mut layout = crate::session_doc::Layout::default();
        layout.unpark(terminal, to)?;
        sessions.insert(session.to_owned(), layout);
    }
    for (name, other) in sessions.iter_mut() {
        if name != session {
            other.remove(terminal);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
