use std::collections::HashSet;

use anyhow::{Context, Result};
use tracing::{debug, warn};

use arb_types::pool::object_id_from_hex;
use dex_common::{parse_type_params, parse_type_params_with_fee, PoolDeserializer};
use sui_client::ObjectDataOptions;

use crate::PoolManager;

impl PoolManager {
    /// Discover Cetus pools from the pools registry.
    pub(crate) async fn discover_cetus_pools(
        &self,
        whitelisted: &HashSet<&str>,
    ) -> Result<usize> {
        let registry_id = &self.config.cetus.pools_registry;
        let cetus_type_prefix = &self.config.cetus.package_types;

        // Step 1: Enumerate all pool IDs from the registry's dynamic fields
        let mut pool_ids = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = self
                .client
                .get_dynamic_fields(registry_id, cursor, Some(50))
                .await
                .context("enumerate Cetus pools registry")?;

            for field in &page.data {
                pool_ids.push(field.object_id.clone());
            }

            if !page.has_next_page {
                break;
            }
            cursor = page.next_cursor;
        }

        debug!(count = pool_ids.len(), "found Cetus registry entries");

        // The registry entries are LinkedTable nodes containing pool IDs.
        // Fetch the registry entries to extract actual pool object IDs.
        let actual_pool_ids = self
            .extract_pool_ids_from_registry_entries(&pool_ids)
            .await?;

        debug!(
            count = actual_pool_ids.len(),
            "extracted Cetus pool IDs from registry"
        );

        // Step 2: Batch-fetch pool objects with BCS
        let mut count = 0;
        for chunk in actual_pool_ids.chunks(50) {
            let objects = self
                .client
                .multi_get_objects(chunk, ObjectDataOptions::bcs())
                .await
                .context("batch fetch Cetus pools")?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };

                // Only process Cetus pool types
                let type_str = match data.bcs_type() {
                    Some(t) if t.contains(&format!("{}::pool::Pool", cetus_type_prefix)) => t,
                    _ => continue,
                };

                let type_params = parse_type_params(type_str);
                if type_params.len() < 2 {
                    continue;
                }

                // Filter by whitelisted tokens
                if !whitelisted.is_empty()
                    && !whitelisted.contains(type_params[0].as_str())
                    && !whitelisted.contains(type_params[1].as_str())
                {
                    continue;
                }

                let bcs_bytes = match data.bcs_bytes() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("skip Cetus pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let object_id = match object_id_from_hex(&data.object_id) {
                    Ok(id) => id,
                    Err(e) => {
                        warn!("skip Cetus pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let version = data.version_number();
                let isv = data.initial_shared_version().unwrap_or(0);

                match dex_cetus::CetusDeserializer::deserialize_pool(
                    object_id,
                    &bcs_bytes,
                    &type_params,
                    version,
                    isv,
                ) {
                    Ok(pool) if pool.is_active => {
                        self.insert_pool(pool);
                        count += 1;
                    }
                    Ok(_) => {
                        debug!("skip paused Cetus pool {}", data.object_id);
                    }
                    Err(e) => {
                        debug!("skip Cetus pool {} deser error: {}", data.object_id, e);
                    }
                }
            }
        }

        Ok(count)
    }

    /// Discover Turbos pools from the pool table.
    pub(crate) async fn discover_turbos_pools(
        &self,
        whitelisted: &HashSet<&str>,
    ) -> Result<usize> {
        let table_id = &self.config.turbos.pool_table_id;
        let turbos_type_prefix = &self.config.turbos.package_types;

        // Step 1: Enumerate all pool IDs from the pool table
        let mut pool_ids = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = self
                .client
                .get_dynamic_fields(table_id, cursor, Some(50))
                .await
                .context("enumerate Turbos pool table")?;

            for field in &page.data {
                pool_ids.push(field.object_id.clone());
            }

            if !page.has_next_page {
                break;
            }
            cursor = page.next_cursor;
        }

        debug!(count = pool_ids.len(), "found Turbos table entries");

        // For Turbos, the table entries may directly be the pool objects
        // or contain references to them. Fetch them to find out.
        let actual_pool_ids = self
            .extract_pool_ids_from_table_entries(&pool_ids, turbos_type_prefix)
            .await?;

        debug!(
            count = actual_pool_ids.len(),
            "extracted Turbos pool IDs"
        );

        // Step 2: Batch-fetch pool objects
        let mut count = 0;
        for chunk in actual_pool_ids.chunks(50) {
            let objects = self
                .client
                .multi_get_objects(chunk, ObjectDataOptions::bcs())
                .await
                .context("batch fetch Turbos pools")?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };

                let type_str = match data.bcs_type() {
                    Some(t) if t.contains(&format!("{}::pool::Pool", turbos_type_prefix)) => t,
                    _ => continue,
                };

                let (coin_params, fee_type) = parse_type_params_with_fee(type_str);
                if coin_params.len() < 2 {
                    continue;
                }

                // Filter by whitelisted tokens
                if !whitelisted.is_empty()
                    && !whitelisted.contains(coin_params[0].as_str())
                    && !whitelisted.contains(coin_params[1].as_str())
                {
                    continue;
                }

                let bcs_bytes = match data.bcs_bytes() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("skip Turbos pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let object_id = match object_id_from_hex(&data.object_id) {
                    Ok(id) => id,
                    Err(e) => {
                        warn!("skip Turbos pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                // Build full type_params including fee type
                let mut type_params = coin_params;
                if let Some(ft) = fee_type {
                    type_params.push(ft);
                }

                let version = data.version_number();
                let isv = data.initial_shared_version().unwrap_or(0);

                match dex_turbos::TurbosDeserializer::deserialize_pool(
                    object_id,
                    &bcs_bytes,
                    &type_params,
                    version,
                    isv,
                ) {
                    Ok(pool) if pool.is_active => {
                        self.insert_pool(pool);
                        count += 1;
                    }
                    Ok(_) => {
                        debug!("skip locked Turbos pool {}", data.object_id);
                    }
                    Err(e) => {
                        debug!("skip Turbos pool {} deser error: {}", data.object_id, e);
                    }
                }
            }
        }

        Ok(count)
    }

    /// Extract actual pool object IDs from Cetus LinkedTable registry entries.
    ///
    /// The registry is a LinkedTable<ID, PoolSimpleInfo>. Each dynamic field
    /// contains a node with the pool ID. We fetch them with content to extract IDs.
    async fn extract_pool_ids_from_registry_entries(
        &self,
        entry_ids: &[String],
    ) -> Result<Vec<String>> {
        let mut pool_ids = Vec::new();

        for chunk in entry_ids.chunks(50) {
            let objects = self
                .client
                .multi_get_objects(chunk, ObjectDataOptions::content())
                .await
                .context("fetch Cetus registry entries")?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };

                // Try to extract pool ID from content.
                // LinkedTable node fields contain "value" which has the pool info.
                if let Some(content) = &data.content {
                    // The content has fields.value which may contain the pool ID.
                    // Try multiple paths since the exact structure may vary.
                    if let Some(pool_id) = extract_id_from_content(content) {
                        pool_ids.push(pool_id);
                    } else {
                        // If the entry itself is the pool, use its ID
                        pool_ids.push(data.object_id.clone());
                    }
                } else {
                    pool_ids.push(data.object_id.clone());
                }
            }
        }

        Ok(pool_ids)
    }

    /// Extract pool IDs from Turbos table entries.
    async fn extract_pool_ids_from_table_entries(
        &self,
        entry_ids: &[String],
        turbos_prefix: &str,
    ) -> Result<Vec<String>> {
        let mut pool_ids = Vec::new();

        for chunk in entry_ids.chunks(50) {
            let objects = self
                .client
                .multi_get_objects(chunk, ObjectDataOptions::bcs())
                .await
                .context("fetch Turbos table entries")?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };

                // Check if this IS a pool object directly
                if let Some(type_str) = data.bcs_type() {
                    if type_str.contains(&format!("{}::pool::Pool", turbos_prefix)) {
                        pool_ids.push(data.object_id.clone());
                        continue;
                    }
                }

                // Otherwise try to extract pool ID from content
                if let Some(content) = &data.content {
                    if let Some(pool_id) = extract_id_from_content(content) {
                        pool_ids.push(pool_id);
                        continue;
                    }
                }

                // Fallback: the entry might be wrapped, use its ID
                pool_ids.push(data.object_id.clone());
            }
        }

        Ok(pool_ids)
    }
}

/// Try to extract a pool object ID from dynamic field content JSON.
fn extract_id_from_content(content: &serde_json::Value) -> Option<String> {
    // Path 1: content.fields.value (LinkedTable node value)
    if let Some(id) = content
        .get("fields")
        .and_then(|f| f.get("value"))
        .and_then(|v| v.as_str())
    {
        return Some(id.to_string());
    }

    // Path 2: content.fields.value.fields.id (nested struct)
    if let Some(id) = content
        .get("fields")
        .and_then(|f| f.get("value"))
        .and_then(|v| v.get("fields"))
        .and_then(|f| f.get("id"))
        .and_then(|v| v.as_str())
    {
        return Some(id.to_string());
    }

    // Path 3: content.fields.name (the key itself is the pool ID)
    if let Some(id) = content
        .get("fields")
        .and_then(|f| f.get("name"))
        .and_then(|v| v.as_str())
    {
        if id.starts_with("0x") && id.len() > 10 {
            return Some(id.to_string());
        }
    }

    None
}
