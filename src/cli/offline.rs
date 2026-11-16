use crate::api::schema::{
    AgentStatus, ErrorBody, ErrorResponse, Method, Request, ResponseResult, SuccessResponse,
    TabInfo, WorkspaceInfo,
};
use crate::persist::{SessionSnapshot, WorkspaceSnapshot};
use crate::workspace::{decode_public_number, derive_label_from_cwd, public_tab_id_for_number};

pub(super) fn owns_the_target() -> bool {
    !super::target::is_remote() && crate::session::uses_own_socket()
}

type Answer = Option<Result<ResponseResult, ErrorBody>>;

pub(super) fn handle(request: &Request) -> Option<std::io::Result<serde_json::Value>> {
    let answer = match &request.method {
        Method::WorkspaceList(_) => crate::persist::doc::edit(|snapshot| {
            let answer: Answer = (0..snapshot.workspaces.len())
                .map(|index| workspace_info(snapshot, index))
                .collect::<Option<Vec<_>>>()
                .map(|workspaces| Ok(ResponseResult::WorkspaceList { workspaces }));
            (answer, false)
        }),
        Method::WorkspaceRename(params) => crate::persist::doc::edit(|snapshot| {
            let Some(index) = workspace_index(snapshot, &params.workspace_id) else {
                return (Some(Err(workspace_not_found(&params.workspace_id))), false);
            };
            snapshot.workspaces[index].custom_name = Some(params.label.clone());
            let answer: Answer = workspace_info(snapshot, index)
                .map(|workspace| Ok(ResponseResult::WorkspaceInfo { workspace }));
            let changed = answer.is_some();
            (answer, changed)
        }),
        Method::TabList(params) => crate::persist::doc::edit(|snapshot| {
            let workspaces = match params.workspace_id.as_deref() {
                Some(workspace_id) => match workspace_index(snapshot, workspace_id) {
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
        Method::TabRename(params) => crate::persist::doc::edit(|snapshot| {
            let Some((workspace, tab)) = tab_index(snapshot, &params.tab_id) else {
                return (Some(Err(tab_not_found(&params.tab_id))), false);
            };
            snapshot.workspaces[workspace].tabs[tab].custom_name = Some(params.label.clone());
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

fn workspace_index(snapshot: &SessionSnapshot, id: &str) -> Option<usize> {
    snapshot
        .workspaces
        .iter()
        .position(|workspace| workspace.id.as_deref() == Some(id))
        .or_else(|| id.strip_prefix("w_")?.parse::<usize>().ok()?.checked_sub(1))
        .or_else(|| id.parse::<usize>().ok()?.checked_sub(1))
        .filter(|&index| index < snapshot.workspaces.len())
}

fn tab_index(snapshot: &SessionSnapshot, id: &str) -> Option<(usize, usize)> {
    let visible = |workspace: usize, tab: usize| {
        (tab < snapshot.workspaces[workspace].visible_tab_count()).then_some((workspace, tab))
    };
    if let Some(rest) = id.strip_prefix("t_") {
        let (workspace_raw, tab_raw) = rest.rsplit_once('_')?;
        let workspace = workspace_index(snapshot, workspace_raw)?;
        return visible(workspace, tab_raw.parse::<usize>().ok()?.checked_sub(1)?);
    }
    let (workspace_raw, tab_raw) = id.rsplit_once(':')?;
    let workspace = workspace_index(snapshot, workspace_raw)?;
    let tab = match tab_raw.strip_prefix('t') {
        Some(encoded) => {
            let number = decode_public_number(encoded)?;
            let workspace = &snapshot.workspaces[workspace];
            (0..workspace.tabs.len()).position(|tab| workspace.tab_number(tab) == number)?
        }
        None => tab_raw.parse::<usize>().ok()?.checked_sub(1)?,
    };
    visible(workspace, tab)
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
        pane_count: workspace.public_pane_numbers.len(),
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
        pane_count: workspace.tabs[tab].panes.len(),
        agent_status: AgentStatus::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::{LayoutSnapshot, TabSnapshot};

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
    fn ids_resolve_the_way_the_server_resolves_them() {
        let snapshot = snapshot();
        assert_eq!(workspace_index(&snapshot, "w1"), Some(0));
        assert_eq!(workspace_index(&snapshot, "1"), Some(0));
        assert_eq!(workspace_index(&snapshot, "w_1"), Some(0));
        assert_eq!(workspace_index(&snapshot, "2"), None);
        assert_eq!(tab_index(&snapshot, "w1:t3"), Some((0, 1)));
        assert_eq!(tab_index(&snapshot, "w1:2"), Some((0, 1)));
        assert_eq!(tab_index(&snapshot, "t_w1_2"), Some((0, 1)));
        assert_eq!(
            tab_index(&snapshot, "w1:t4"),
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
    fn a_rename_without_a_server_lands_in_the_document() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
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
        let name = crate::persist::doc::edit(|_| ((), false)).ok();
        assert_eq!(name, Some(None));
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
