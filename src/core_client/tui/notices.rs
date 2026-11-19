use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::session::Session;
use super::terminals::{Moved, Status};
use crate::api::schema::AgentStatus;
use crate::app::actions::{
    active_tab_suppresses_notifications, notification_toast_for_state_change, sound_for_toast_kind,
    toast_event_text,
};
use crate::app::ToastKind;
use crate::config::{Config, SoundConfig, ToastDelivery, ToastHerdrPosition};
use crate::detect::Agent;
use crate::protocol::{CellData, SemanticNotificationKind};
use crate::sound::Sound;
use crate::ui::theme::Palette;

const QUEUED: usize = 8;

pub(super) struct Notifying {
    pub(super) delivery: ToastDelivery,
    pub(super) delay: Duration,
    pub(super) position: ToastHerdrPosition,
    pub(super) sound: SoundConfig,
}

impl Notifying {
    pub(super) fn from_config(config: &Config) -> Self {
        Self {
            delivery: config.ui.toast.delivery,
            delay: Duration::from_secs(config.ui.toast.delay_seconds),
            position: config.ui.toast.herdr.position,
            sound: config.ui.sound.clone(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    Sound(Sound),
    Terminal { title: String, body: String },
    System { title: String, body: String },
}

pub(super) struct Notice {
    terminal: String,
    agent: Agent,
    kind: ToastKind,
    body: String,
    at: Instant,
}

impl Notice {
    fn title(&self) -> String {
        format!(
            "{} {}",
            crate::detect::agent_label(self.agent),
            toast_event_text(self.kind)
        )
    }

    fn semantic(&self) -> SemanticNotificationKind {
        match self.kind {
            ToastKind::NeedsAttention => SemanticNotificationKind::NeedsAttention,
            ToastKind::Finished => SemanticNotificationKind::Finished,
            ToastKind::UpdateInstalled => SemanticNotificationKind::UpdateInstalled,
        }
    }

    fn current(&self, status: Option<&Status>) -> bool {
        status.is_some_and(|status| {
            status.agent == Some(self.agent)
                && matches!(
                    (self.kind, status.shown()),
                    (ToastKind::NeedsAttention, AgentStatus::Blocked)
                        | (ToastKind::Finished, AgentStatus::Done)
                )
        })
    }

    pub(super) fn image(
        &self,
        host: Rect,
        mobile: bool,
        position: ToastHerdrPosition,
        palette: &Palette,
    ) -> (Rect, Vec<CellData>) {
        let mut buffer = Buffer::empty(host);
        let dot = match self.kind {
            ToastKind::NeedsAttention => palette.red,
            ToastKind::Finished => palette.blue,
            ToastKind::UpdateInstalled => palette.accent,
        };
        let rect = match mobile {
            true => self.banner(&mut buffer, host, dot, palette),
            false => crate::client::render_notification_card(
                &mut buffer,
                host,
                &self.title(),
                &self.body,
                position,
                0,
                dot,
                palette,
            ),
        };
        let cells = (rect.y..rect.bottom())
            .flat_map(|y| (rect.x..rect.right()).map(move |x| (x, y)))
            .map(|position| CellData::from_ratatui_cell(&buffer[position]))
            .collect();
        (rect, cells)
    }

    fn banner(&self, buffer: &mut Buffer, host: Rect, dot: Color, palette: &Palette) -> Rect {
        let agent = crate::detect::agent_label(self.agent);
        let title = match self.kind {
            ToastKind::NeedsAttention => format!("{agent} waiting"),
            ToastKind::Finished => format!("{agent} done"),
            ToastKind::UpdateInstalled => "update ready".to_owned(),
        };
        crate::client::render_mobile_notice_banner(
            buffer,
            host,
            &title,
            Some(&self.body),
            dot,
            false,
            palette,
        )
    }
}

#[derive(Default)]
pub(super) struct Notices {
    notifying: Option<Notifying>,
    pending: Vec<Notice>,
    shown: Option<Notice>,
    queued: VecDeque<Notice>,
    shows: u64,
}

impl Notices {
    pub(super) fn new(notifying: Notifying) -> Self {
        Self {
            notifying: Some(notifying),
            ..Self::default()
        }
    }

    pub(super) fn position(&self) -> ToastHerdrPosition {
        self.notifying
            .as_ref()
            .map_or_else(ToastHerdrPosition::default, |notifying| notifying.position)
    }

    pub(super) fn shown(&self) -> Option<&Notice> {
        self.shown.as_ref()
    }

    pub(super) fn shows(&self) -> u64 {
        self.shows
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .chain(&self.shown)
            .map(|notice| notice.at)
            .min()
    }

    pub(super) fn hear(&mut self, moved: Moved, session: &Session, now: Instant) {
        let Some(notifying) = &self.notifying else {
            return;
        };
        let (Some(agent), Some(kind)) = (
            moved.agent,
            notification_toast_for_state_change(false, moved.from, moved.to),
        ) else {
            return;
        };
        let Some(body) = session.context(&moved.terminal) else {
            return;
        };
        let at = now.checked_add(notifying.delay).unwrap_or(now);
        let other = |notice: &Notice| notice.terminal != moved.terminal;
        self.pending.retain(other);
        self.queued.retain(other);
        if self.shown.as_ref().is_some_and(|notice| !other(notice)) {
            self.promote(now);
        }
        self.pending.push(Notice {
            terminal: moved.terminal,
            agent,
            kind,
            body,
            at,
        });
    }

    pub(super) fn deliver(
        &mut self,
        session: &Session,
        host_focused: bool,
        now: Instant,
    ) -> Vec<Effect> {
        if self.shown.as_ref().is_some_and(|notice| now >= notice.at) {
            self.promote(now);
        }
        let due: Vec<_> = self
            .pending
            .extract_if(.., |notice| notice.at <= now)
            .collect();
        let mut effects = Vec::new();
        for notice in due {
            let Some(notifying) = &self.notifying else {
                break;
            };
            if !notice.current(session.status(&notice.terminal)) {
                continue;
            }
            let active = session.in_focused_tab(&notice.terminal);
            let suppress = active_tab_suppresses_notifications(active, Some(host_focused));
            if let Some(sound) = sound_for_toast_kind(notice.kind, suppress)
                .filter(|_| notifying.sound.allows(Some(notice.agent)))
            {
                effects.push(Effect::Sound(sound));
            }
            match notifying.delivery {
                ToastDelivery::Herdr if !active => self.queue(notice, now),
                ToastDelivery::Terminal if !suppress => effects.push(Effect::Terminal {
                    title: notice.title(),
                    body: notice.body,
                }),
                ToastDelivery::System if !suppress => effects.push(Effect::System {
                    title: notice.title(),
                    body: notice.body,
                }),
                _ => {}
            }
        }
        effects
    }

    pub(super) fn open(&mut self, now: Instant) -> Option<String> {
        self.shown.as_ref()?;
        self.promote(now).map(|notice| notice.terminal)
    }

    fn queue(&mut self, notice: Notice, now: Instant) {
        if self.shown.is_none() {
            self.show(notice, now);
            return;
        }
        if self.queued.len() == QUEUED {
            self.queued.pop_front();
        }
        self.queued.push_back(notice);
    }

    fn promote(&mut self, now: Instant) -> Option<Notice> {
        let replaced = self.shown.take();
        self.shows += 1;
        if let Some(next) = self.queued.pop_front() {
            self.show(next, now);
        }
        replaced
    }

    fn show(&mut self, mut notice: Notice, now: Instant) {
        let lasts = crate::client::notification_duration(notice.semantic());
        notice.at = now.checked_add(lasts).unwrap_or(now);
        self.shows += 1;
        self.shown = Some(notice);
    }
}
