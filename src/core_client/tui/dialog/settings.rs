use crossterm::event::{
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use super::{Hits, Outcome};
use crate::config::{Config, ConfigEdit, StatusIndicatorStyle, ToastDelivery, THEME_NAMES};
use crate::input::TerminalKey;
use crate::ui::dialog::{normalized_theme_name, SettingsSection};
use crate::ui::theme::{Palette, ThemeRuntimeConfig};

const SECTIONS: &[SettingsSection] = &[
    SettingsSection::Theme,
    SettingsSection::Indicators,
    SettingsSection::Sound,
    SettingsSection::Toast,
];
const INDICATORS: [StatusIndicatorStyle; 2] =
    [StatusIndicatorStyle::Dots, StatusIndicatorStyle::Symbols];
const SOUNDS: [bool; 2] = [true, false];
const TOASTS: [ToastDelivery; 4] = [
    ToastDelivery::Off,
    ToastDelivery::Herdr,
    ToastDelivery::Terminal,
    ToastDelivery::System,
];

#[derive(Clone, Debug)]
pub(in crate::core_client::tui) struct Chosen {
    theme: ThemeRuntimeConfig,
    indicators: StatusIndicatorStyle,
    sound: bool,
    toasts: ToastDelivery,
    previewed: Option<usize>,
}

impl From<&Config> for Chosen {
    fn from(config: &Config) -> Self {
        Self {
            theme: crate::ui::theme::client_theme_runtime_from_config(config),
            indicators: config.ui.status_indicators,
            sound: config.ui.sound.enabled,
            toasts: config.ui.toast.delivery,
            previewed: None,
        }
    }
}

impl Chosen {
    pub(in crate::core_client::tui) fn palette(&self, theme: usize) -> Option<Palette> {
        let name = THEME_NAMES.get(theme)?;
        Some(crate::ui::theme::client_palette_for_theme(
            &self.theme,
            name,
        ))
    }

    pub(in crate::core_client::tui) fn preview(&mut self, theme: Option<usize>) {
        self.previewed = theme;
    }

    fn shown(&self, section: SettingsSection) -> usize {
        match (section, self.previewed) {
            (SettingsSection::Theme, Some(theme)) => theme,
            _ => self.index(section),
        }
    }

    fn index(&self, section: SettingsSection) -> usize {
        self.position(section).unwrap_or(0)
    }

    fn position(&self, section: SettingsSection) -> Option<usize> {
        let theme = normalized_theme_name(&self.theme.manual_name);
        match section {
            SettingsSection::Theme => THEME_NAMES
                .iter()
                .position(|name| normalized_theme_name(name) == theme),
            SettingsSection::Indicators => INDICATORS.iter().position(|i| *i == self.indicators),
            SettingsSection::Sound => SOUNDS.iter().position(|on| *on == self.sound),
            SettingsSection::Toast => TOASTS.iter().position(|to| *to == self.toasts),
            SettingsSection::Integrations => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::core_client::tui) struct Preferences {
    section: SettingsSection,
    selected: usize,
}

impl Preferences {
    pub(in crate::core_client::tui) fn new(chosen: &Chosen) -> Self {
        Self {
            section: SettingsSection::Theme,
            selected: chosen.index(SettingsSection::Theme),
        }
    }

    pub(super) fn saved(&mut self, chosen: &Chosen, written: bool) -> Outcome {
        if !written
            || Some(self.selected) != chosen.position(self.section)
            || self.section == SettingsSection::Theme && chosen.theme.auto_switch
        {
            self.selected = chosen.index(self.section);
            return Outcome::Stay;
        }
        if self.section == SettingsSection::Theme {
            return Outcome::Close;
        }
        Outcome::Stay
    }

    pub(super) fn key(&mut self, key: &TerminalKey, hits: &Hits, chosen: &Chosen) -> Outcome {
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        let plain = modifiers.is_empty();
        match code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if plain => self.turn(1, chosen),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h')
                if modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                self.turn(-1, chosen);
            }
            KeyCode::Up | KeyCode::Char('k') if plain => return self.step(-1),
            KeyCode::Down | KeyCode::Char('j') if plain => return self.step(1),
            KeyCode::Enter | KeyCode::Char(' ') if plain && key.kind != KeyEventKind::Repeat => {
                return self.save(hits);
            }
            _ => {}
        }
        Outcome::Stay
    }

    pub(super) fn mouse(
        &mut self,
        event: MouseEvent,
        point: Position,
        hits: &Hits,
        chosen: &Chosen,
    ) -> Outcome {
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Outcome::Stay;
        }
        if let Some((_, section)) = hits.tabs.iter().find(|(rect, _)| rect.contains(point)) {
            self.section = *section;
            self.selected = chosen.shown(*section);
            return Outcome::Stay;
        }
        if let Some((_, (section, index))) =
            hits.choices.iter().find(|(rect, _)| rect.contains(point))
        {
            self.section = *section;
            self.selected = *index;
            if *section == SettingsSection::Theme {
                return Outcome::Preview(*index);
            }
            return self.save(hits);
        }
        if hits.primary.contains(point) {
            return self.save(hits);
        }
        if hits.cancel.contains(point) || !hits.area.contains(point) {
            return Outcome::Close;
        }
        Outcome::Stay
    }

    pub(super) fn draw(
        &self,
        buffer: &mut Buffer,
        chosen: &Chosen,
        palette: &Palette,
    ) -> Option<Hits> {
        let drawn = crate::ui::dialog::render_settings(
            buffer,
            &crate::ui::dialog::Settings {
                section: self.section,
                selected: self.selected,
                sections: SECTIONS,
                original_theme_name: &chosen.theme.manual_name,
                integrations: &[],
                integration_messages: &[],
                loading_integrations: false,
                installing_integrations: false,
                integration_updates_available: false,
            },
            palette,
        )?;
        Some(Hits {
            area: drawn.area,
            primary: drawn.primary,
            cancel: drawn.close,
            tabs: drawn
                .tabs
                .into_iter()
                .filter(|(rect, _)| rect.width > 1)
                .collect(),
            choices: drawn
                .choices
                .into_iter()
                .map(|(rect, index)| (rect, (self.section, index)))
                .collect(),
            ..Hits::default()
        })
    }

    fn turn(&mut self, by: isize, chosen: &Chosen) {
        let at = SECTIONS
            .iter()
            .position(|section| *section == self.section)
            .unwrap_or(0);
        let next = (at + SECTIONS.len()).saturating_add_signed(by) % SECTIONS.len();
        self.section = SECTIONS[next];
        self.selected = chosen.shown(self.section);
    }

    fn step(&mut self, by: isize) -> Outcome {
        let count = match self.section {
            SettingsSection::Theme => THEME_NAMES.len(),
            SettingsSection::Indicators => INDICATORS.len(),
            SettingsSection::Sound => SOUNDS.len(),
            SettingsSection::Toast => TOASTS.len(),
            SettingsSection::Integrations => 1,
        };
        self.selected = self
            .selected
            .saturating_add_signed(by)
            .min(count.saturating_sub(1));
        match self.section {
            SettingsSection::Theme => Outcome::Preview(self.selected),
            _ => Outcome::Stay,
        }
    }

    fn save(&self, hits: &Hits) -> Outcome {
        let drawn = hits
            .choices
            .iter()
            .any(|(_, choice)| *choice == (self.section, self.selected));
        let edit = match self.section {
            SettingsSection::Theme => THEME_NAMES
                .get(self.selected)
                .map(|name| ConfigEdit::Theme(name)),
            SettingsSection::Indicators => INDICATORS
                .get(self.selected)
                .map(|style| ConfigEdit::StatusIndicators(*style)),
            SettingsSection::Sound => SOUNDS.get(self.selected).map(|on| ConfigEdit::Sound(*on)),
            SettingsSection::Toast => TOASTS
                .get(self.selected)
                .map(|to| ConfigEdit::ToastDelivery(*to)),
            SettingsSection::Integrations => None,
        };
        match edit {
            Some(edit) if drawn => Outcome::Save(edit),
            _ => Outcome::Stay,
        }
    }
}
