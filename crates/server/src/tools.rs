use serde_json::{Value, json};
use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
};

use crate::rpc::{BitcoinRpc, RpcCallError};

#[derive(Clone)]
pub struct BitcoinRpcNostrServer {
    tool_router: ToolRouter<Self>,
    rpc: Arc<BitcoinRpc>,
}

impl BitcoinRpcNostrServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            rpc: Arc::new(BitcoinRpc::from_env()),
        }
    }
}

impl Default for BitcoinRpcNostrServer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetBlockParams {
    /// Block hash (hex)
    blockhash: String,
    /// Verbosity: 0=hex, 1=json, 2=json with tx details
    #[serde(default)]
    verbosity: Option<u8>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetBlockHashParams {
    /// Block height
    height: u64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetRawMempoolParams {
    /// If true, return detailed info for each tx; otherwise just an array of txids
    #[serde(default)]
    verbose: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetRawTransactionParams {
    /// Transaction id (hex)
    txid: String,
    /// Verbosity: 0=hex, 1=json, 2=json with fee/prevout details
    #[serde(default)]
    verbosity: Option<u8>,
    /// Optional block hash (hex) the transaction is contained in
    #[serde(default)]
    blockhash: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetBlockHeaderParams {
    /// Block hash (hex)
    blockhash: String,
    /// If true, return decoded JSON; otherwise the raw hex header
    #[serde(default)]
    verbose: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetBlockHeaderRawParams {
    /// Block hash (hex)
    blockhash: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetBlockFilterParams {
    /// Block hash (hex)
    blockhash: String,
    /// Filter type (e.g. "basic")
    #[serde(default)]
    filtertype: Option<String>,
}

#[tool_router]
impl BitcoinRpcNostrServer {
    #[tool(
        name = "getblockchaininfo",
        description = "Get current blockchain state (height, chain, difficulty, ...)"
    )]
    async fn get_blockchain_info(&self) -> Result<CallToolResult, ErrorData> {
        let result = self
            .rpc
            .call("getblockchaininfo", json!([]))
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getnetworkinfo",
        description = "Get network state (version, connections, relay fee, ...)"
    )]
    async fn get_network_info(&self) -> Result<CallToolResult, ErrorData> {
        let result = self
            .rpc
            .call("getnetworkinfo", json!([]))
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(name = "getblock", description = "Get a block by hash")]
    async fn get_block(
        &self,
        Parameters(GetBlockParams {
            blockhash,
            verbosity,
        }): Parameters<GetBlockParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!([blockhash, verbosity.unwrap_or(1)]);
        let result = self
            .rpc
            .call("getblock", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblockcount",
        description = "Get the height of the most-work fully-validated chain"
    )]
    async fn get_block_count(&self) -> Result<CallToolResult, ErrorData> {
        let result = self
            .rpc
            .call("getblockcount", json!([]))
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblockhash",
        description = "Get the block hash at a given height"
    )]
    async fn get_block_hash(
        &self,
        Parameters(GetBlockHashParams { height }): Parameters<GetBlockHashParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = self
            .rpc
            .call("getblockhash", json!([height]))
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getrawmempool",
        description = "Get the transaction ids in the mempool (or detailed info when verbose)"
    )]
    async fn get_raw_mempool(
        &self,
        Parameters(GetRawMempoolParams { verbose }): Parameters<GetRawMempoolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = self
            .rpc
            .call("getrawmempool", json!([verbose.unwrap_or(false)]))
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getrawtransaction",
        description = "Get a raw transaction by txid"
    )]
    async fn get_raw_transaction(
        &self,
        Parameters(GetRawTransactionParams {
            txid,
            verbosity,
            blockhash,
        }): Parameters<GetRawTransactionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut p = vec![json!(txid), json!(verbosity.unwrap_or(1))];
        if let Some(bh) = blockhash {
            p.push(json!(bh));
        }
        let params = Value::Array(p);

        let result = self
            .rpc
            .call("getrawtransaction", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblock_verbose",
        description = "Get a block by hash (decoded); variant of getblock"
    )]
    async fn get_block_info(
        &self,
        Parameters(GetBlockParams {
            blockhash,
            verbosity,
        }): Parameters<GetBlockParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!([blockhash, verbosity.unwrap_or(1)]);
        let result = self
            .rpc
            .call("getblock", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblockheader",
        description = "Get a block header by hash (decoded JSON or raw hex)"
    )]
    async fn get_block_header_info(
        &self,
        Parameters(GetBlockHeaderParams { blockhash, verbose }): Parameters<GetBlockHeaderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!([blockhash, verbose.unwrap_or(true)]);
        let result = self
            .rpc
            .call("getblockheader", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblockheader_hex",
        description = "Get the raw hex-encoded block header by hash"
    )]
    async fn get_block_header(
        &self,
        Parameters(GetBlockHeaderRawParams { blockhash }): Parameters<GetBlockHeaderRawParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!([blockhash, false]);
        let result = self
            .rpc
            .call("getblockheader", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        name = "getblockfilter",
        description = "Get the BIP157 content filter for a block by hash"
    )]
    async fn get_block_filter(
        &self,
        Parameters(GetBlockFilterParams {
            blockhash,
            filtertype,
        }): Parameters<GetBlockFilterParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!([blockhash, filtertype.unwrap_or_else(|| "basic".to_string())]);
        let result = self
            .rpc
            .call("getblockfilter", params)
            .await
            .map_err(RpcCallError::into_error_data)?;

        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }
}

/// Normalize a tool name for lenient matching: lowercase and drop underscores.
/// This lets clients call tools using either Bitcoin Core style
/// (`getblockhash`) or snake_case (`get_block_hash`).
fn normalize_tool_name(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BitcoinRpcNostrServer {
    async fn call_tool(
        &self,
        mut request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let requested = request.name.to_string();

        // Resolve the requested tool name to a canonical route. Bitcoin Core
        // style names (`getblockhash`) are canonical, but we also accept
        // snake_case / mixed-case variants (`get_block_hash`, `getBlockHash`)
        // by matching on a normalized form.
        let canonical = if self.tool_router.has_route(&requested) {
            Some(requested.clone())
        } else {
            let wanted = normalize_tool_name(&requested);
            self.tool_router
                .map
                .keys()
                .find(|name| normalize_tool_name(name) == wanted)
                .map(|name| name.to_string())
        };

        let Some(canonical) = canonical else {
            tracing::warn!(tool = %requested, "tool not found");
            return Err(ErrorData::invalid_params(
                format!("tool not found: {requested}"),
                Some(json!({ "tool": requested })),
            ));
        };

        if canonical != requested {
            tracing::debug!(requested = %requested, canonical = %canonical, "resolved tool alias");
            request.name = std::borrow::Cow::Owned(canonical);
        }

        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_info(&self) -> rmcp::model::ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("ContextBTC — Bitcoin Core over MCP/Nostr"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_tool_name;

    #[test]
    fn normalizes_case_and_underscores() {
        // Bitcoin Core style, snake_case, and mixed case all collapse to the
        // same canonical form.
        assert_eq!(normalize_tool_name("getblockhash"), "getblockhash");
        assert_eq!(normalize_tool_name("get_block_hash"), "getblockhash");
        assert_eq!(normalize_tool_name("getBlockHash"), "getblockhash");
        assert_eq!(normalize_tool_name("GET_BLOCK_HASH"), "getblockhash");
    }

    #[test]
    fn distinct_names_stay_distinct() {
        assert_ne!(
            normalize_tool_name("getblock"),
            normalize_tool_name("getblockheader")
        );
    }
}
