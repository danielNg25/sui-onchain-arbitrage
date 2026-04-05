use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Options for what data to include in object responses.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectDataOptions {
    pub show_type: bool,
    pub show_owner: bool,
    pub show_bcs: bool,
    pub show_content: bool,
    pub show_previous_transaction: bool,
    pub show_storage_rebate: bool,
    pub show_display: bool,
}

impl ObjectDataOptions {
    pub fn bcs() -> Self {
        Self {
            show_type: true,
            show_owner: true,
            show_bcs: true,
            show_content: false,
            show_previous_transaction: false,
            show_storage_rebate: false,
            show_display: false,
        }
    }

    pub fn content() -> Self {
        Self {
            show_type: true,
            show_owner: true,
            show_bcs: false,
            show_content: true,
            show_previous_transaction: false,
            show_storage_rebate: false,
            show_display: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SuiObjectResponse {
    pub data: Option<SuiObjectData>,
    pub error: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiObjectData {
    pub object_id: String,
    pub version: String,
    pub digest: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub bcs: Option<SuiRawData>,
    pub owner: Option<Value>,
    pub content: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "dataType")]
pub enum SuiRawData {
    #[serde(rename = "moveObject")]
    #[serde(rename_all = "camelCase")]
    MoveObject {
        bcs_bytes: String,
        #[serde(rename = "type")]
        type_: String,
        has_public_transfer: bool,
        version: Value,
    },
    #[serde(rename = "package")]
    Package {
        #[serde(flatten)]
        _rest: Value,
    },
}

impl SuiObjectData {
    /// Extract base64-decoded BCS bytes from the response.
    pub fn bcs_bytes(&self) -> Result<Vec<u8>, anyhow::Error> {
        match &self.bcs {
            Some(SuiRawData::MoveObject { bcs_bytes, .. }) => {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(bcs_bytes)
                    .map_err(|e| anyhow::anyhow!("base64 decode failed: {}", e))
            }
            _ => Err(anyhow::anyhow!("no BCS data in object response")),
        }
    }

    /// Get the Move type string from BCS data.
    pub fn bcs_type(&self) -> Option<&str> {
        match &self.bcs {
            Some(SuiRawData::MoveObject { type_, .. }) => Some(type_),
            _ => None,
        }
    }

    /// Extract `initial_shared_version` from owner field if this is a shared object.
    pub fn initial_shared_version(&self) -> Option<u64> {
        self.owner.as_ref().and_then(|owner| {
            owner
                .get("Shared")
                .and_then(|shared| shared.get("initial_shared_version"))
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        })
    }

    pub fn version_number(&self) -> u64 {
        self.version.parse().unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicFieldPage {
    pub data: Vec<DynamicFieldInfo>,
    pub next_cursor: Option<String>,
    pub has_next_page: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicFieldInfo {
    pub name: DynamicFieldName,
    #[serde(rename = "objectId")]
    pub object_id: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub bcs_name: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldName {
    #[serde(rename = "type")]
    pub type_: String,
    pub value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub data: Vec<SuiEvent>,
    pub next_cursor: Option<EventCursor>,
    pub has_next_page: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCursor {
    pub tx_digest: String,
    pub event_seq: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiEvent {
    pub id: EventCursor,
    pub package_id: String,
    pub transaction_module: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub parsed_json: Option<Value>,
    pub bcs: Option<String>,
    pub sender: String,
    pub timestamp_ms: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum EventFilter {
    MoveEventType(String),
    MoveModule {
        package: String,
        module: String,
    },
}

/// Options for transaction response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TxResponseOptions {
    pub show_input: bool,
    pub show_effects: bool,
    pub show_events: bool,
    pub show_object_changes: bool,
    pub show_balance_changes: bool,
    pub show_raw_effects: bool,
    pub show_raw_input: bool,
}

impl TxResponseOptions {
    pub fn effects_and_events() -> Self {
        Self {
            show_input: false,
            show_effects: true,
            show_events: true,
            show_object_changes: false,
            show_balance_changes: false,
            show_raw_effects: false,
            show_raw_input: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiTxResponse {
    pub digest: String,
    pub effects: Option<Value>,
    pub events: Option<Vec<SuiEvent>>,
    #[serde(flatten)]
    pub rest: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevInspectResults {
    pub effects: Value,
    pub results: Option<Vec<DevInspectResult>>,
    pub error: Option<String>,
    pub events: Vec<SuiEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevInspectResult {
    pub mutable_reference_outputs: Option<Value>,
    pub return_values: Option<Vec<(Vec<u8>, String)>>,
}
