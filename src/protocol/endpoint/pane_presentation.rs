use serde::{Deserialize, Serialize};

use crate::protocol::ServerMessage;

pub const CAPABILITY: &str = "pane_presentation_v1";
pub const SET_KIND: &str = "endpoint.pane-presentation.set.v1";
pub const IDENTITIES_KIND: &str = "endpoint.pane-presentation.v1";
pub const ERROR_KIND: &str = "endpoint.pane-presentation.error.v1";

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_TERMINALS: usize = 4096;

/// Replaces this connection's presentation policy; it never edits session topology.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPanePresentation {
    pub boot_id: String,
    pub sequence: u64,
    #[serde(default)]
    pub hidden_terminals: Vec<String>,
    #[serde(default)]
    pub read_only_terminals: Vec<String>,
    #[serde(default)]
    pub preview_terminal: Option<String>,
    #[serde(default)]
    pub focus_terminal: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneIdentities {
    pub boot_id: String,
    pub revision: u64,
    pub sequence: u64,
    pub pane_terminals: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanePresentationError {
    pub boot_id: String,
    pub sequence: Option<u64>,
    pub code: String,
    pub message: String,
}

pub fn decode_set(data: &str) -> Result<SetPanePresentation, &'static str> {
    if data.len() > MAX_REQUEST_BYTES {
        return Err("pane presentation request is too large");
    }
    let request: SetPanePresentation =
        serde_json::from_str(data).map_err(|_| "invalid pane presentation request")?;
    if request.hidden_terminals.len() > MAX_TERMINALS
        || request.read_only_terminals.len() > MAX_TERMINALS
    {
        return Err("too many terminals in pane presentation request");
    }
    if request
        .hidden_terminals
        .iter()
        .chain(&request.read_only_terminals)
        .chain(request.preview_terminal.iter())
        .chain(request.focus_terminal.iter())
        .any(|id| id.is_empty() || id.len() > 128)
    {
        return Err("invalid terminal identity in pane presentation request");
    }
    Ok(request)
}

pub fn identities_message(identities: &PaneIdentities) -> serde_json::Result<ServerMessage> {
    Ok(ServerMessage::EndpointControl {
        kind: IDENTITIES_KIND.into(),
        data: serde_json::to_string(identities)?,
    })
}

pub fn error_message(error: &PanePresentationError) -> serde_json::Result<ServerMessage> {
    Ok(ServerMessage::EndpointControl {
        kind: ERROR_KIND.into(),
        data: serde_json::to_string(error)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_policy_fields_default_and_unknown_fields_are_ignored() {
        let request = decode_set(r#"{"boot_id":"boot","sequence":1,"future":true}"#).unwrap();
        assert!(request.hidden_terminals.is_empty());
        assert!(request.read_only_terminals.is_empty());
        assert!(request.preview_terminal.is_none());
    }

    #[test]
    fn malformed_or_unbounded_policies_are_rejected() {
        assert!(decode_set("not json").is_err());
        let mut request = decode_set(r#"{"boot_id":"boot","sequence":1}"#).unwrap();
        request.hidden_terminals = vec![String::from("term"); MAX_TERMINALS + 1];
        assert!(decode_set(&serde_json::to_string(&request).unwrap()).is_err());
        request.hidden_terminals = vec![String::new()];
        assert!(decode_set(&serde_json::to_string(&request).unwrap()).is_err());
        assert!(decode_set(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }
}
