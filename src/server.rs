use serde_json::{Value, json};
use std::sync::Arc;

use contextvm_sdk::signer;
use contextvm_sdk::transport::server::{NostrServerTransport, NostrServerTransportConfig};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
};

#[derive(Clone)]
pub struct BitcoinRpcNostrServer {
    tool_router: ToolRouter<Self>,
    rpc: Arc<BitcoinRpc>,
}

impl BitcoinRpcNostrServer {
    pub fn new() -> Self {
        let timeout_secs = std::env::var("BITCOIN_RPC_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
        let rpc = BitcoinRpc {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .expect("failed to build HTTP client"),
            url: std::env::var("BITCOIN_RPC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8332".to_string()),
            user: std::env::var("BITCOIN_RPC_USER").unwrap_or_default(),
            password: std::env::var("BITCOIN_RPC_PASSWORD").unwrap_or_default(),
        };
        Self {
            tool_router: Self::tool_router(),
            rpc: Arc::new(rpc),
        }
    }
}
struct BitcoinRpc {
    http: reqwest::Client,
    url: String, // e.g. "http://127.0.0.1:8332"
    user: String,
    password: String,
}

/// An error from a single RPC attempt, tagged with whether it is worth retrying.
struct RpcCallError {
    retryable: bool,
    /// Whether `source` is safe to relay to clients verbatim. Errors derived
    /// from a `reqwest::Error` can contain infrastructure details (e.g. the
    /// node URL) and are not client-safe; messages we construct ourselves are.
    client_safe: bool,
    source: anyhow::Error,
}

impl RpcCallError {
    /// A transport-level error derived from `reqwest`. May contain the node
    /// URL, so it is not surfaced to clients verbatim.
    fn transport(retryable: bool, source: anyhow::Error) -> Self {
        Self {
            retryable,
            client_safe: false,
            source,
        }
    }

    /// An error whose message we constructed ourselves (contains no secrets),
    /// safe to surface to clients.
    fn safe(retryable: bool, source: anyhow::Error) -> Self {
        Self {
            retryable,
            client_safe: true,
            source,
        }
    }

    /// Convert into a client-facing MCP error, sanitizing anything that could
    /// leak infrastructure details. Non-safe errors are logged server-side.
    fn into_error_data(self) -> ErrorData {
        if self.client_safe {
            ErrorData::internal_error(self.source.to_string(), None)
        } else {
            tracing::error!(error = %self.source, "bitcoind request failed");
            ErrorData::internal_error(
                "failed to query the Bitcoin node (see server logs)".to_string(),
                None,
            )
        }
    }
}

impl BitcoinRpc {
    /// Perform an RPC call, retrying transient failures (connect/timeout errors
    /// and 5xx/429 responses) with exponential backoff. Permanent failures
    /// (auth errors, malformed responses, application-level RPC errors) fail
    /// immediately.
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcCallError> {
        use backon::{ExponentialBuilder, Retryable};

        let policy = ExponentialBuilder::default()
            .with_max_times(3)
            .with_jitter();

        (|| async { self.call_once(method, &params).await })
            .retry(policy)
            .when(|e: &RpcCallError| e.retryable)
            .notify(|e: &RpcCallError, dur| {
                tracing::warn!(
                    method = %method,
                    error = %e.source,
                    retry_in = ?dur,
                    "bitcoind RPC call failed; retrying"
                );
            })
            .await
    }

    async fn call_once(&self, method: &str, params: &Value) -> Result<Value, RpcCallError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "context-btc",
            "method": method,
            "params": params,
        });

        let resp = self
            .http
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                // Connection and timeout failures are typically transient.
                let retryable = e.is_timeout() || e.is_connect();
                RpcCallError::transport(retryable, e.into())
            })?;

        // Read the status and body once. bitcoind returns non-JSON bodies
        // (often plain text or HTML) on transport-level errors, so we must not
        // blindly parse as JSON.
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| RpcCallError::transport(true, e.into()))?;

        if !status.is_success() {
            let hint = match status {
                reqwest::StatusCode::UNAUTHORIZED => {
                    " (check BITCOIN_RPC_USER / BITCOIN_RPC_PASSWORD)"
                }
                reqwest::StatusCode::FORBIDDEN => {
                    " (client not allowed; check bitcoind rpcallowip / rpcbind)"
                }
                _ => "",
            };
            let snippet: String = text.trim().chars().take(200).collect();
            // Server errors and rate limiting are transient; 4xx are not.
            let retryable =
                status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let err = anyhow::anyhow!("bitcoind HTTP {status}{hint}: {snippet}");
            return Err(RpcCallError::safe(retryable, err));
        }

        let resp: Value = serde_json::from_str(&text).map_err(|_| {
            let snippet: String = text.trim().chars().take(200).collect();
            RpcCallError::safe(
                false,
                anyhow::anyhow!("bitcoind returned a non-JSON response: {snippet}"),
            )
        })?;

        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            return Err(RpcCallError::safe(
                false,
                anyhow::anyhow!("bitcoind RPC error: {err}"),
            ));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
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

pub async fn run_server() -> anyhow::Result<()> {
    let signer = match std::env::var("NOSTR_SECRET_KEY") {
        Ok(sk) => signer::from_sk(&sk)?,
        Err(_) => {
            eprintln!(
                "WARNING: NOSTR_SECRET_KEY not set; generating an ephemeral key \
                 (identity will change on every restart)."
            );
            signer::generate()
        }
    };
    let pubkey = signer.public_key().to_hex();
    println!("Public key: {pubkey}");

    let transport = NostrServerTransport::new(
        signer,
        NostrServerTransportConfig::default()
            .with_relay_urls(vec![
                "ws://localhost:10547".to_string(),
                // "wss://relay.contextvm.org".to_string(),
                // "wss://nos.lol".to_string(),
            ])
            .with_announced_server(false),
    )
    .await?;

    let service = BitcoinRpcNostrServer::new().serve(transport).await?;
    println!("Server ready. Press Ctrl+C to stop.");
    service.waiting().await?;
    Ok(())
}
