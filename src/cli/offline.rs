use crate::api::schema::{
    AgentStatus, ErrorBody, ErrorResponse, Method, Request, ResponseResult, SuccessResponse,
    TabInfo, WorkspaceInfo,
};
use crate::persist::{SessionSnapshot, WorkspaceSnapshot};
use crate::session_model::workspace_index;
use crate::workspace::{derive_label_from_cwd, public_tab_id_for_number};

pub(super) fn owns_the_target() -> bool {
    !super::target::is_remote() && crate::session::uses_own_socket()
}

type Answer = Option<Result<ResponseResult, ErrorBody>>;

pub(super) fn handle(request: &Request) -> Option<std::io::Result<serde_json::Value>> {
    let answer = match &request.method {
        Method::WorkspaceList(_) => crate::persist::doc::edit(false, |snapshot| {
            let answer: Answer = (0..snapshot.workspaces.len())
                .map(|index| workspace_info(snapshot, index))
                .collect::<Option<Vec<_>>>()
                .map(|workspaces| Ok(ResponseResult::WorkspaceList { workspaces }));
            (answer, false)
        }),
        Method::WorkspaceRename(params) => crate::persist::doc::edit(true, |snapshot| {
            let Some(index) = crate::session_model::rename_workspace(
                &mut snapshot.workspaces,
                &params.workspace_id,
                &params.label,
            ) else {
                return (Some(Err(workspace_not_found(&params.workspace_id))), false);
            };
            let answer: Answer = workspace_info(snapshot, index)
                .map(|workspace| Ok(ResponseResult::WorkspaceInfo { workspace }));
            let changed = answer.is_some();
            (answer, changed)
        }),
        Method::TabList(params) => crate::persist::doc::edit(false, |snapshot| {
            let workspaces = match params.workspace_id.as_deref() {
                Some(workspace_id) => match workspace_index(&snapshot.workspaces, workspace_id) {
                    Some(index) => index..index + 1,
                    None => return (Some(Err(workspace_not_found(workspace_id))), false),
                },
                None => 0..snapshot.workspaces.len(),
            };
            let answer: Answer = workspaces
                .flat_map(|index| {
                    (0..snapshot.workspaces[index].visible_tab_count()).map(move |tab| (index, tab))
                })
                .map(|(index, tab)| tab_info(snapshot, index, tab))
                .collect::<Option<Vec<_>>>()
                .map(|tabs| Ok(ResponseResult::TabList { tabs }));
            (answer, false)
        }),
        Method::TabRename(params) => crate::persist::doc::edit(true, |snapshot| {
            let Some((workspace, tab)) = crate::session_model::rename_tab(
                &mut snapshot.workspaces,
                &params.tab_id,
                &params.label,
            ) else {
                return (Some(Err(tab_not_found(&params.tab_id))), false);
            };
            let answer: Answer =
                tab_info(snapshot, workspace, tab).map(|tab| Ok(ResponseResult::TabInfo { tab }));
            let changed = answer.is_some();
            (answer, changed)
        }),
        _ => return None,
    };
    let id = request.id.clone();
    let response = match answer.map(Option::flatten) {
        Ok(Some(Ok(result))) => serde_json::to_value(SuccessResponse { id, result }),
        Ok(Some(Err(error))) => serde_json::to_value(ErrorResponse { id, error }),
        Ok(None) => return None,
        Err(err) => return Some(Err(std::io::Error::other(err))),
    };
    Some(response.map_err(std::io::Error::other))
}

fn workspace_not_found(workspace_id: &str) -> ErrorBody {
    ErrorBody {
        code: "workspace_not_found".into(),
        message: format!("workspace {workspace_id} not found"),
    }
}

fn tab_not_found(tab_id: &str) -> ErrorBody {
    ErrorBody {
        code: "tab_not_found".into(),
        message: format!("tab {tab_id} not found"),
    }
}

fn active_tab(workspace: &WorkspaceSnapshot) -> usize {
    workspace
        .active_tab
        .min(workspace.tabs.len().saturating_sub(1))
}

fn workspace_info(snapshot: &SessionSnapshot, index: usize) -> Option<WorkspaceInfo> {
    let workspace = &snapshot.workspaces[index];
    let id = workspace.id.clone()?;
    let active_tab = active_tab(workspace);
    let active_tab_number = if active_tab < workspace.visible_tab_count() {
        workspace.tab_number(active_tab)
    } else {
        workspace.next_tab_number()
    };
    Some(WorkspaceInfo {
        number: index + 1,
        label: workspace
            .custom_name
            .clone()
            .unwrap_or_else(|| derive_label_from_cwd(&workspace.identity_cwd)),
        focused: snapshot.active == Some(index),
        pane_count: workspace
            .tabs
            .iter()
            .map(|tab| tab.layout.pane_count())
            .sum(),
        tab_count: workspace.visible_tab_count(),
        active_tab_id: public_tab_id_for_number(&id, active_tab_number),
        agent_status: AgentStatus::Unknown,
        tokens: Default::default(),
        worktree: None,
        parked: workspace.parked && workspace.tabs.iter().any(|tab| tab.parked_from.is_some()),
        workspace_id: id,
    })
}

fn tab_info(snapshot: &SessionSnapshot, workspace_index: usize, tab: usize) -> Option<TabInfo> {
    let workspace = &snapshot.workspaces[workspace_index];
    let id = workspace.id.clone()?;
    let number = workspace.tab_number(tab);
    Some(TabInfo {
        tab_id: public_tab_id_for_number(&id, number),
        workspace_id: id,
        number,
        label: workspace.tabs[tab]
            .custom_name
            .clone()
            .unwrap_or_else(|| (tab + 1).to_string()),
        focused: snapshot.active == Some(workspace_index) && active_tab(workspace) == tab,
        pane_count: workspace.tabs[tab].layout.pane_count(),
        agent_status: AgentStatus::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::{LayoutSnapshot, TabSnapshot};
    use crate::session_model::tab_index;

    fn tab(name: Option<&str>, parked: bool) -> TabSnapshot {
        TabSnapshot {
            custom_name: name.map(str::to_owned),
            layout: LayoutSnapshot::Pane(1),
            panes: Default::default(),
            zoomed: false,
            focused: None,
            root_pane: None,
            parked_from: parked.then_some(1),
        }
    }

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            version: 0,
            workspaces: vec![WorkspaceSnapshot {
                id: Some("w1".into()),
                custom_name: None,
                identity_cwd: "/tmp/project".into(),
                public_pane_numbers: Default::default(),
                next_public_pane_number: 0,
                public_tab_numbers: vec![1, 3, 4],
                next_public_tab_number: 5,
                tabs: vec![tab(None, false), tab(Some("docs"), false), tab(None, true)],
                active_tab: 1,
                parked: false,
            }],
            active: Some(0),
            selected: 0,
            sidebar_width: None,
            sidebar_section_split: None,
            collapsed_space_keys: Default::default(),
        }
    }

    #[test]
    fn live_and_saved_identity_resolution_match() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state = crate::app::AppState::test_with_adversarial_identity_state();
        app.state.assert_invariants_for_test();
        let mut alias = crate::workspace::Workspace::test_new("alias");
        alias.id = "w_1".into();
        app.state.workspaces.push(alias);
        let mut numeric = crate::workspace::Workspace::test_new("numeric");
        numeric.id = "1".into();
        app.state.workspaces.push(numeric);
        app.state.ensure_test_terminals();

        for parked in [false, true] {
            let last = app.state.workspaces[0].tabs.last_mut().unwrap();
            last.parked_from = parked.then_some(1);
            let snapshot = crate::persist::capture(
                &app.state.workspaces,
                &app.state.terminals,
                &app.terminal_runtimes,
                app.state.active,
                app.state.selected,
            );
            let ids = app
                .state
                .workspaces
                .iter()
                .map(|workspace| workspace.id.clone())
                .chain(["0", "2", "3", "4", "w_0", "w_2", "w_4", "missing"].map(String::from));
            for id in ids {
                assert_eq!(
                    workspace_index(&snapshot.workspaces, &id),
                    app.parse_workspace_id(&id),
                    "{id}"
                );
                for number in 0..8 {
                    for tab_id in [
                        format!("{id}:t{}", crate::workspace::encode_public_number(number)),
                        format!("{id}:{number}"),
                        format!("t_{id}_{number}"),
                    ] {
                        assert_eq!(
                            tab_index(&snapshot.workspaces, &tab_id),
                            app.parse_tab_id(&tab_id),
                            "{tab_id}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn legacy_snapshot_identity_reads_do_not_allocate_ids() {
        let mut snapshot = snapshot();
        snapshot.workspaces[0].id = None;
        assert_eq!(workspace_index(&snapshot.workspaces, "1"), Some(0));
        assert_eq!(workspace_index(&snapshot.workspaces, "w_1"), Some(0));
        assert_eq!(workspace_index(&snapshot.workspaces, "w1"), None);
        assert_eq!(tab_index(&snapshot.workspaces, "1:t3"), Some((0, 1)));
        assert_eq!(tab_index(&snapshot.workspaces, "1:t4"), None);
        assert!(workspace_info(&snapshot, 0).is_none());
        snapshot.workspaces[0].tabs.clear();
        assert_eq!(tab_index(&snapshot.workspaces, "1:1"), None);
        assert!(snapshot.workspaces[0].id.is_none());
    }

    #[test]
    fn saved_ids_keep_public_numbers_and_restore_defaults() {
        let snapshot = snapshot();
        assert_eq!(workspace_index(&snapshot.workspaces, "w1"), Some(0));
        assert_eq!(workspace_index(&snapshot.workspaces, "1"), Some(0));
        assert_eq!(workspace_index(&snapshot.workspaces, "w_1"), Some(0));
        assert_eq!(workspace_index(&snapshot.workspaces, "2"), None);
        assert_eq!(tab_index(&snapshot.workspaces, "w1:t3"), Some((0, 1)));
        assert_eq!(tab_index(&snapshot.workspaces, "w1:2"), Some((0, 1)));
        assert_eq!(tab_index(&snapshot.workspaces, "t_w1_2"), Some((0, 1)));
        assert_eq!(
            tab_index(&snapshot.workspaces, "w1:t4"),
            None,
            "parked tabs are hidden"
        );

        let info = tab_info(&snapshot, 0, 1).expect("tab");
        assert_eq!(
            (info.tab_id.as_str(), info.label.as_str()),
            ("w1:t3", "docs")
        );
        assert!(info.focused);
        let workspace = workspace_info(&snapshot, 0).expect("workspace");
        assert_eq!(
            (workspace.label.as_str(), workspace.tab_count),
            ("project", 2)
        );
        assert_eq!(workspace.active_tab_id, "w1:t3");

        let mut stale = snapshot;
        stale.workspaces[0].active_tab = 2;
        stale.workspaces[0].next_public_tab_number = 0;
        let workspace = workspace_info(&stale, 0).expect("workspace");
        assert_eq!(
            workspace.active_tab_id, "w1:t5",
            "the tab restore would number next"
        );

        stale.workspaces[0].tabs.pop();
        stale.workspaces[0].active_tab = 9;
        stale.workspaces[0].parked = true;
        let workspace = workspace_info(&stale, 0).expect("workspace");
        assert_eq!(
            workspace.active_tab_id, "w1:t3",
            "restore clamps the active tab"
        );
        assert!(
            !workspace.parked,
            "restore unparks a workspace with no parked tab"
        );
        assert!(tab_info(&stale, 0, 1).expect("tab").focused);
    }

    #[test]
    fn an_all_parked_workspace_uses_effective_fallback_tab_numbers() {
        let mut snapshot = snapshot();
        let workspace = &mut snapshot.workspaces[0];
        workspace.tabs = vec![tab(None, true), tab(None, true)];
        workspace.public_tab_numbers.clear();
        workspace.next_public_tab_number = 0;
        workspace.active_tab = 0;
        assert_eq!(workspace_info(&snapshot, 0).unwrap().active_tab_id, "w1:t3");
    }

    #[test]
    fn pane_counts_follow_layout_leaves_not_optional_metadata() {
        for (public, metadata) in [
            (vec![], vec![]),
            (vec![(1, 1)], vec![1]),
            (vec![(1, 1), (2, 2), (99, 99)], vec![1, 2, 99]),
        ] {
            let mut snapshot = snapshot();
            let workspace = &mut snapshot.workspaces[0];
            workspace.public_pane_numbers = public.into_iter().collect();
            workspace.tabs[0].layout = LayoutSnapshot::Split {
                direction: crate::persist::DirectionSnapshot::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutSnapshot::Pane(1)),
                second: Box::new(LayoutSnapshot::Pane(2)),
            };
            workspace.tabs[1].layout = LayoutSnapshot::Pane(0);
            workspace.tabs[2].layout = LayoutSnapshot::Pane(3);
            workspace.tabs[0].panes = metadata
                .into_iter()
                .map(|id| {
                    let pane =
                        serde_json::from_value(serde_json::json!({"cwd": "/missing"})).unwrap();
                    (id, pane)
                })
                .collect();
            assert_eq!(workspace_info(&snapshot, 0).unwrap().pane_count, 4);
            assert_eq!(tab_info(&snapshot, 0, 0).unwrap().pane_count, 2);
            assert_eq!(tab_info(&snapshot, 0, 1).unwrap().pane_count, 1);
        }
        let mut snapshot = snapshot();
        snapshot.workspaces[0].tabs.clear();
        assert_eq!(workspace_info(&snapshot, 0).unwrap().pane_count, 0);
    }

    #[test]
    fn a_socket_override_names_another_server() {
        std::env::remove_var(crate::api::SOCKET_PATH_ENV_VAR);
        assert!(owns_the_target());
        std::env::set_var(crate::api::SOCKET_PATH_ENV_VAR, "/tmp/other.sock");
        assert!(!owns_the_target());
        let args = ["lawn", "--session", "work", "workspace", "list"].map(String::from);
        crate::session::configure_from_args(&args).expect("session");
        assert!(owns_the_target(), "--session names its own server");
    }

    #[cfg(unix)]
    #[test]
    fn file_backed_commands_list_while_owned_and_rename_after_release() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        std::env::set_var(crate::session::SESSION_ENV_VAR, "work");
        assert!(crate::persist::load().is_none());
        let mut saved = snapshot();
        saved.collapsed_space_keys.insert("x".repeat(1024 * 1024));
        crate::persist::SessionWriter::new(false).save(&saved, None);
        let request = |method| Request {
            id: "cli".into(),
            method,
        };
        let rename = request(Method::WorkspaceRename(
            crate::api::schema::WorkspaceRenameParams {
                workspace_id: "w1".into(),
                label: "renamed".into(),
            },
        ));
        let owner = crate::session::own_session(Some("work")).unwrap();
        let listed =
            super::super::send_request(&request(Method::WorkspaceList(Default::default())))
                .unwrap();
        assert_eq!(listed["result"]["workspaces"][0]["pane_count"], 3);
        assert!(super::super::send_request(&rename).is_err());
        drop(owner);
        let renamed = super::super::send_request(&rename).unwrap();
        assert_eq!(renamed["result"]["workspace"]["pane_count"], 3);
        let renamed_tab = super::super::send_request(&request(Method::TabRename(
            crate::api::schema::TabRenameParams {
                tab_id: "w1:t3".into(),
                label: "tab renamed".into(),
            },
        )))
        .unwrap();
        assert_eq!(renamed_tab["result"]["tab"]["pane_count"], 1);
        let tabs =
            super::super::send_request(&request(Method::TabList(Default::default()))).unwrap();
        assert_eq!(tabs["result"]["tabs"].as_array().unwrap().len(), 2);
        assert_eq!(
            test.core
                .doc_get(&crate::persist::doc::name(), None)
                .unwrap()
                .value["lawn_layout_file"]["version"],
            1
        );
        let loaded = crate::persist::load().unwrap();
        assert_eq!(loaded.workspaces[0].custom_name.as_deref(), Some("renamed"));
        assert_eq!(
            loaded.workspaces[0].tabs[1].custom_name.as_deref(),
            Some("tab renamed")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_rename_without_a_server_lands_in_the_document() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        std::env::set_var(crate::session::SESSION_ENV_VAR, "work");
        let request = |method| Request {
            id: "cli".into(),
            method,
        };
        let rename = |label: &str| {
            request(Method::WorkspaceRename(
                crate::api::schema::WorkspaceRenameParams {
                    workspace_id: "w1".into(),
                    label: label.into(),
                },
            ))
        };
        assert!(handle(&rename("x")).is_none(), "no layout yet");
        assert!(
            !crate::session::data_dir().exists(),
            "a session without a layout stays absent"
        );
        std::fs::create_dir_all(crate::session::data_dir()).expect("the session ran");
        let value = serde_json::to_value(snapshot()).expect("json");
        test.core
            .doc_set(&crate::persist::doc::name(), None, &value, 0)
            .expect("doc.set");

        let answer = handle(&rename("renamed")).expect("handled").expect("ok");

        assert_eq!(answer["result"]["workspace"]["label"], "renamed");
        let stored = test
            .core
            .doc_get(&crate::persist::doc::name(), None)
            .expect("doc.get");
        assert_eq!(stored.version, 2);
        assert_eq!(stored.value["workspaces"][0]["custom_name"], "renamed");
        let missing = handle(&request(Method::TabRename(
            crate::api::schema::TabRenameParams {
                tab_id: "w1:t9".into(),
                label: "x".into(),
            },
        )))
        .expect("handled")
        .expect("ok");
        assert_eq!(missing["error"]["code"], "tab_not_found");
    }
}
