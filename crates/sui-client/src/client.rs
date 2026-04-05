use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tracing::debug;

use crate::types::*;

pub struct SuiClient {
    http: reqwest::Client,
    rpc_url: String,
    request_id: AtomicU64,
}

impl SuiClient {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            rpc_url: rpc_url.to_string(),
            request_id: AtomicU64::new(1),
        }
    }

    async fn rpc_call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        debug!(method = method, "RPC call");

        let resp = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("RPC request to {} failed", method))?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            anyhow::bail!("RPC {} returned HTTP {}: {}", method, status, text);
        }

        let rpc_resp: Value = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse RPC response for {}", method))?;

        if let Some(error) = rpc_resp.get("error") {
            anyhow::bail!("RPC {} error: {}", method, error);
        }

        let result = rpc_resp
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("RPC {} response missing 'result' field", method))?
            .clone();

        serde_json::from_value(result)
            .with_context(|| format!("failed to deserialize RPC {} result", method))
    }

    /// Fetch a single object by ID.
    pub async fn get_object(
        &self,
        id: &str,
        options: ObjectDataOptions,
    ) -> Result<SuiObjectResponse> {
        self.rpc_call("sui_getObject", json!([id, options])).await
    }

    /// Fetch multiple objects by ID.
    pub async fn multi_get_objects(
        &self,
        ids: &[String],
        options: ObjectDataOptions,
    ) -> Result<Vec<SuiObjectResponse>> {
        self.rpc_call("sui_multiGetObjects", json!([ids, options]))
            .await
    }

    /// List dynamic fields on a parent object.
    pub async fn get_dynamic_fields(
        &self,
        parent_id: &str,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<DynamicFieldPage> {
        self.rpc_call(
            "suix_getDynamicFields",
            json!([parent_id, cursor, limit]),
        )
        .await
    }

    /// Fetch a specific dynamic field object.
    pub async fn get_dynamic_field_object(
        &self,
        parent_id: &str,
        name: &DynamicFieldName,
    ) -> Result<SuiObjectResponse> {
        self.rpc_call(
            "suix_getDynamicFieldObject",
            json!([parent_id, name]),
        )
        .await
    }

    /// Query events with a filter.
    pub async fn query_events(
        &self,
        filter: EventFilter,
        cursor: Option<EventCursor>,
        limit: Option<u32>,
        descending: bool,
    ) -> Result<EventPage> {
        self.rpc_call(
            "suix_queryEvents",
            json!([filter, cursor, limit, descending]),
        )
        .await
    }

    /// Dry-run a transaction via devInspect.
    pub async fn dev_inspect(
        &self,
        sender: &str,
        tx_kind_bytes: &str,
    ) -> Result<DevInspectResults> {
        self.rpc_call(
            "sui_devInspectTransactionBlock",
            json!([sender, tx_kind_bytes]),
        )
        .await
    }

    /// Execute a signed transaction.
    pub async fn execute_tx(
        &self,
        tx_bytes: &str,
        signatures: &[String],
        options: TxResponseOptions,
    ) -> Result<SuiTxResponse> {
        self.rpc_call(
            "sui_executeTransactionBlock",
            json!([tx_bytes, signatures, options, "WaitForEffects"]),
        )
        .await
    }

    /// Get the current reference gas price.
    pub async fn get_reference_gas_price(&self) -> Result<u64> {
        let result: String = self
            .rpc_call("suix_getReferenceGasPrice", json!([]))
            .await?;
        result
            .parse()
            .with_context(|| format!("failed to parse gas price '{}'", result))
    }

    /// Get the latest checkpoint sequence number.
    pub async fn get_latest_checkpoint_sequence_number(&self) -> Result<u64> {
        let result: String = self
            .rpc_call("sui_getLatestCheckpointSequenceNumber", json!([]))
            .await?;
        result
            .parse()
            .with_context(|| format!("failed to parse checkpoint number '{}'", result))
    }
}
