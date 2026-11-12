use serde::{Deserialize, Serialize};

pub use crate::terminal::background::{BackgroundEvent, BackgroundSnapshot};

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
}
