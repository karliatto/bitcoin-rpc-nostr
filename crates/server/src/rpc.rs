use serde_json::{Value, json};

use rmcp::model::ErrorData;

pub struct BitcoinRpc {
    http: reqwest::Client,
    url: String, // e.g. "http://127.0.0.1:8332"
    user: String,
    password: String,
}

impl BitcoinRpc {
    /// Build the RPC client from environment variables, applying sensible
    /// defaults for the URL and timeout.
    pub fn from_env() -> Self {
        let timeout_secs = std::env::var("BITCOIN_RPC_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .expect("failed to build HTTP client"),
            url: std::env::var("BITCOIN_RPC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8332".to_string()),
            user: std::env::var("BITCOIN_RPC_USER").unwrap_or_default(),
            password: std::env::var("BITCOIN_RPC_PASSWORD").unwrap_or_default(),
        }
    }

    /// Perform an RPC call, retrying transient failures (connect/timeout errors
    /// and 5xx/429 responses) with exponential backoff. Permanent failures
    /// (auth errors, malformed responses, application-level RPC errors) fail
    /// immediately.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcCallError> {
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
            "id": "contextbtc",
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

/// An error from a single RPC attempt, tagged with whether it is worth retrying.
pub struct RpcCallError {
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
    pub fn into_error_data(self) -> ErrorData {
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
