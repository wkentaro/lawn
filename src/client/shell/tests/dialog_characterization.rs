use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn bounds(rect: Rect) -> [u16; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

fn projected() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.update_available = Some("1.2.3".into());
    projected.integration_updates_available = true;
    projected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        active_tab_id: "tab_2".into(),
        new_workspace_cwd: "/other".into(),
        number: 2,
        label: "解析 workspace".into(),
        custom_label: true,
        branch: None,
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: None,
        focused: false,
        agent_status: AgentStatus::Working,
        parked: false,
    });
    projected.tabs.push(ClientShellTab {
        tab_id: "tab_2".into(),
        workspace_id: "ws_2".into(),
        number: 1,
        label: "build".into(),
        custom_label: true,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Working,
    });
    projected.panes.push(ClientShellPane {
        pane_id: "pane_2".into(),
        workspace_id: "ws_2".into(),
        tab_id: "tab_2".into(),
        label: Some("server".into()),
        cwd: Some("/other".into()),
        foreground_cwd: Some("/other".into()),
        focused: false,
        right_click_passthrough: true,
    });
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_2".into(),
        workspace_id: "ws_2".into(),
        tab_id: "tab_2".into(),
        name: Some("pi".into()),
        display_agent: Some("pi".into()),
        agent: Some("pi".into()),
        title: Some("Review".into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    projected
}

fn integration(
    target: crate::api::schema::IntegrationTarget,
    label: &str,
    available: bool,
    state: crate::api::schema::IntegrationState,
) -> crate::api::schema::IntegrationInfo {
    crate::api::schema::IntegrationInfo {
        target,
        label: label.into(),
        command: label.into(),
        available,
        state,
    }
}

fn open(state: &mut ClientShellState, name: &str) {
    use crate::api::schema::{IntegrationState, IntegrationTarget};

    let kind = name.trim_end_matches("_small").trim_end_matches("_reset");
    let edited = |text: &str, replace: bool| {
        let mut editor = TextEditor::new(text, replace);
        editor.handle_key(&crate::input::TerminalKey::new(
            KeyCode::Left,
            crossterm::event::KeyModifiers::NONE,
        ));
        editor
    };
    state.overlay = Some(match kind {
        "confirm_close" => {
            state.open_confirm_close_overlay("ws_2".into());
            return;
        }
        "rename" => ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            input: edited("client-shell 模块 e\u{301}", false),
            target: ClientRenameTarget::Workspace {
                workspace_id: "ws_1".into(),
            },
        }),
        "new_tab" => ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "new tab",
            input: TextEditor::new("2", true),
            target: ClientRenameTarget::NewTab {
                workspace_id: "ws_1".into(),
                default_name: "2".into(),
            },
        }),
        "help" => ClientShellOverlay::Help(ClientHelpOverlay {
            query: TextEditor::default(),
            search_focused: false,
            scroll: 3,
        }),
        "help_search" => ClientShellOverlay::Help(ClientHelpOverlay {
            query: edited("split", false),
            search_focused: true,
            scroll: 0,
        }),
        "help_empty" => ClientShellOverlay::Help(ClientHelpOverlay {
            query: TextEditor::from("zzzz"),
            search_focused: true,
            scroll: 0,
        }),
        "navigator" => {
            state.open_navigator_overlay();
            return;
        }
        "navigator_search" => {
            state.open_navigator_overlay();
            state.insert_overlay_text("serv");
            return;
        }
        "move_pane" => {
            state.open_move_overlay(ClientMoveSource::Pane("pane_1".into()));
            return;
        }
        "move_tab" => {
            state.open_move_overlay(ClientMoveSource::Tab {
                tab_id: "tab_1".into(),
                workspace_id: "ws_1".into(),
            });
            return;
        }
        "workspace_menu" => {
            state.open_workspace_context_menu("ws_1".into(), 3, 4);
            return;
        }
        "tab_menu" => {
            state.open_tab_context_menu("tab_1".into(), 70, 40);
            return;
        }
        "pane_menu" => {
            state.open_pane_context_menu("pane_2".into(), 30, 2);
            if let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() {
                menu.highlighted = 2;
            }
            return;
        }
        "global_menu" => ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay { highlighted: 1 }),
        "settings_theme" | "settings_indicators" | "settings_toast" | "settings_integrations"
        | "settings_loading" => {
            let section = match kind {
                "settings_theme" => ClientSettingsSection::Theme,
                "settings_indicators" => ClientSettingsSection::Indicators,
                "settings_toast" => ClientSettingsSection::Toast,
                _ => ClientSettingsSection::Integrations,
            };
            ClientShellOverlay::Settings(ClientSettingsOverlay {
                section,
                selected: if section == ClientSettingsSection::Theme {
                    14
                } else {
                    1
                },
                original_theme_name: "Tokyo Night".into(),
                original_palette: Palette::catppuccin(),
                integrations: if kind == "settings_integrations" {
                    vec![
                        integration(IntegrationTarget::Claude, "claude", true, IntegrationState::Current),
                        integration(IntegrationTarget::Codex, "codex", true, IntegrationState::Outdated),
                        integration(IntegrationTarget::Pi, "pi", true, IntegrationState::NotInstalled),
                        integration(
                            IntegrationTarget::Opencode,
                            "opencode",
                            false,
                            IntegrationState::NotInstalled,
                        ),
                    ]
                } else {
                    Vec::new()
                },
                integration_messages: if kind == "settings_integrations" {
                    vec!["codex: updated".into(), "pi: installed".into()]
                } else {
                    Vec::new()
                },
                loading_integrations: kind == "settings_loading",
                installing_integrations: kind == "settings_integrations",
            })
        }
        "onboarding" => ClientShellOverlay::Onboarding,
        "release_notes" => {
            ClientShellOverlay::ReleaseNotes(crate::app::state::ReleaseNotesState {
                version: "1.2.3".into(),
                body: "## Fixed\n\n- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight\n- nine\n- ten\n- eleven\n- twelve".into(),
                scroll: 2,
                preview: true,
            })
        }
        "announcement" => ClientShellOverlay::ProductAnnouncement(
            crate::app::state::ProductAnnouncementState {
                version: "1.2.3".into(),
                id: "news".into(),
                title: "Lawn news".into(),
                body: "A short note.\n\n- first\n- second".into(),
                scroll: 0,
                preview: false,
            },
        ),
        other => panic!("unknown dialog {other}"),
    });
}

fn capture(name: &str) -> Value {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.palette = if name.ends_with("_reset") {
        Palette::terminal()
    } else {
        Palette::catppuccin()
    };
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected()));
    state.set_pane_surface(surface());
    open(&mut state, name);
    let (cols, rows) = if name.ends_with("_small") {
        (40, 10)
    } else {
        (100, 30)
    };
    let frame = state.compose(cols, rows);
    let hits = &state.hits;
    json!({
        "name": name,
        "rows": frame.as_ref().map(|frame| frame_rows(frame)),
        "cursor": frame.as_ref().and_then(|frame| frame.cursor.as_ref()).map(|cursor| [cursor.x, cursor.y, u16::from(cursor.visible), u16::from(cursor.shape)]),
        "frame_sha256": frame.as_ref().map(|frame| format!("{:x}", Sha256::digest(serde_json::to_vec(&**frame).expect("frame JSON")))),
        "hits": {
            "primary": bounds(hits.overlay_primary),
            "clear": bounds(hits.overlay_clear),
            "cancel": bounds(hits.overlay_cancel),
            "context_menu": hits.context_menu_rows.iter().map(|(rect, index)| (bounds(*rect), index)).collect::<Vec<_>>(),
            "global_menu": hits.global_menu_rows.iter().map(|(rect, index)| (bounds(*rect), index)).collect::<Vec<_>>(),
            "navigator_popup": bounds(hits.navigator_popup),
            "navigator_search": bounds(hits.navigator_search),
            "navigator_rows": hits.navigator_rows.iter().map(|(rect, target)| (bounds(*rect), format!("{target:?}"))).collect::<Vec<_>>(),
            "navigator_preview": bounds(hits.navigator_preview),
            "navigator_close": bounds(hits.navigator_close),
            "navigator_scrollbar": bounds(hits.navigator_scrollbar),
            "navigator_scroll": hits.navigator_scroll_metrics.map(|metrics| [metrics.offset_from_bottom, metrics.max_offset_from_bottom, metrics.viewport_rows]),
            "help_popup": bounds(hits.help_popup),
            "help_scrollbar": bounds(hits.help_scrollbar),
            "help_scroll": hits.help_scroll_metrics.map(|metrics| [metrics.offset_from_bottom, metrics.max_offset_from_bottom, metrics.viewport_rows]),
            "help_max_scroll": hits.help_max_scroll,
            "settings_popup": bounds(hits.settings_popup),
            "settings_tabs": hits.settings_tabs.iter().map(|(rect, section)| (bounds(*rect), format!("{section:?}"))).collect::<Vec<_>>(),
            "settings_choices": hits.settings_choices.iter().map(|(rect, index)| (bounds(*rect), index)).collect::<Vec<_>>(),
            "release_notes_scrollbar": bounds(hits.release_notes_scrollbar),
            "release_notes_max_scroll": hits.release_notes_max_scroll,
            "announcement_scrollbar": bounds(hits.product_announcement_scrollbar),
            "announcement_max_scroll": hits.product_announcement_max_scroll,
        },
    })
}

#[test]
fn current_dialogs_match_recorded_cells_and_targets() {
    let actual = Value::Array(
        [
            "confirm_close",
            "confirm_close_small",
            "confirm_close_reset",
            "rename",
            "rename_small",
            "new_tab",
            "help",
            "help_search",
            "help_empty",
            "help_small",
            "navigator",
            "navigator_search",
            "navigator_small",
            "move_pane",
            "move_tab",
            "workspace_menu",
            "tab_menu",
            "pane_menu",
            "pane_menu_small",
            "global_menu",
            "global_menu_reset",
            "settings_theme",
            "settings_indicators",
            "settings_toast",
            "settings_integrations",
            "settings_loading",
            "settings_theme_small",
            "onboarding",
            "release_notes",
            "announcement",
        ]
        .into_iter()
        .map(capture)
        .collect(),
    );
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/dialog-characterization.json"
    ))
    .expect("recorded dialogs");
    if actual != expected {
        eprintln!(
            "DIALOG_CAPTURE={}",
            serde_json::to_string(&actual).expect("capture JSON")
        );
    }
    assert_eq!(actual, expected);
}
