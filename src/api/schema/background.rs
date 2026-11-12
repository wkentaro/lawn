use serde::{Deserialize, Serialize};

pub use crate::terminal::background::{BackgroundEvent, BackgroundSnapshot, ShellObservation};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundReportParams {
    pub pane_id: String,
    pub source: String,
    pub session_id: String,
    pub event: BackgroundEvent,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundShellReportParams {
    pub pane_id: String,
    pub source: String,
    pub session_id: String,
    pub observation: ShellObservation,
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{Method, Request};

    #[test]
    fn background_report_accepts_only_the_observation_contract() {
        let value = serde_json::json!({"id":"hook", "method":"background.report", "params":{
            "pane_id":"w1:p1", "source":"herdr:claude", "session_id":"root",
            "event":"subagent_started", "task_id":"child", "task_type":"Explore"
        }});
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(request.method, Method::BackgroundReport(_)));
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        let mut unsupported = value.clone();
        unsupported["params"]["event"] = "task_completed".into();
        assert!(serde_json::from_value::<Request>(unsupported).is_err());
        let mut future_payload = value;
        future_payload["params"]["snapshot"] = serde_json::json!({});
        assert!(serde_json::from_value::<Request>(future_payload).is_err());
    }

    #[test]
    fn background_shell_report_has_a_strict_nested_observation_envelope() {
        let value = serde_json::json!({"id":"hook", "method":"background.shell.report", "params":{
            "pane_id":"w1:p1", "source":"herdr:claude", "session_id":"root",
            "observation":{"event":"snapshot_observed", "hook_event":"SubagentStop", "hook_agent_id":"reporter",
                "tasks":[{"task_id":"shell", "reported_status":"future-native-status", "command":"echo one\necho two"}], "truncated":false}
        }});
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(request.method, Method::BackgroundShellReport(_)));
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        for (path, invalid) in [
            ("/params/observation", serde_json::Value::Null),
            ("/params/observation/event", "subagent_started".into()),
            ("/params/observation/hook_event", "PostToolUse".into()),
            ("/params/observation/hook_agent_id", 123.into()),
            ("/params/observation/tasks", serde_json::json!({})),
            ("/params/observation/tasks/0", serde_json::Value::Null),
            ("/params/observation/tasks/0/reported_status", false.into()),
            (
                "/params/observation/tasks/0/command",
                serde_json::json!(["output"]),
            ),
            ("/params/observation/truncated", "false".into()),
        ] {
            let mut malformed = value.clone();
            *malformed.pointer_mut(path).unwrap() = invalid;
            assert!(
                serde_json::from_value::<Request>(malformed).is_err(),
                "{path}"
            );
        }
        for (path, field) in [
            ("/params", "event"),
            ("/params/observation", "command"),
            ("/params/observation/tasks/0", "output_path"),
        ] {
            let mut malformed = value.clone();
            malformed.pointer_mut(path).unwrap()[field] = "unexpected".into();
            assert!(
                serde_json::from_value::<Request>(malformed).is_err(),
                "{path}/{field}"
            );
        }
        for field in ["event", "hook_event", "tasks", "truncated"] {
            let mut malformed = value.clone();
            malformed["params"]["observation"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<Request>(malformed).is_err(),
                "missing {field}"
            );
        }
        for duplicate in [
            r#"{"event":"bash_background_observed","task_id":"a","task_id":"b","tool_use_id":"t"}"#,
            r#"{"event":"snapshot_observed","hook_event":"Stop","tasks":[{"task_id":"a","reported_status":"pending","reported_status":"running"}],"truncated":false}"#,
        ] {
            let request = format!(
                r#"{{"id":"hook","method":"background.shell.report","params":{{"pane_id":"w1:p1","source":"herdr:claude","session_id":"root","observation":{duplicate}}}}}"#
            );
            assert!(serde_json::from_str::<Request>(&request).is_err());
        }
        let mut bash = value;
        bash["params"]["observation"] = serde_json::json!({"event":"bash_background_observed", "task_id":"shell", "tool_use_id":"tool"});
        assert!(serde_json::from_value::<Request>(bash.clone()).is_ok());
        bash["params"]["observation"]["tasks"] = serde_json::json!([]);
        assert!(serde_json::from_value::<Request>(bash).is_err());
    }

    #[test]
    fn background_snapshot_without_shell_data_remains_compatible() {
        let value = serde_json::json!({"session_id":null, "coverage":"observations_only", "current_state":"unknown", "records":[], "revision":0, "truncated":false});
        let snapshot: super::BackgroundSnapshot = serde_json::from_value(value.clone()).unwrap();
        assert!(snapshot.shell_observations.is_none());
        assert_eq!(serde_json::to_value(snapshot).unwrap(), value);
    }
}
