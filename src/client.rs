use contextvm_sdk::signer;
use contextvm_sdk::transport::client::{NostrClientTransport, NostrClientTransportConfig};
use rmcp::{ClientHandler, ServiceExt, model::CallToolRequestParams};

#[derive(Clone, Default)]
struct BitcoinRpcNostrClient;
impl ClientHandler for BitcoinRpcNostrClient {}

pub async fn run_client(server_pubkey: String) -> anyhow::Result<()> {
    let signer = signer::generate();
    println!("Client starting. Target Server: {}", server_pubkey);

    let transport = NostrClientTransport::new(
        signer,
        NostrClientTransportConfig::default()
            .with_relay_urls(vec![
                "ws://localhost:10547".to_string(),
                // "wss://relay.contextvm.org".to_string(),
                // "wss://nos.lol".to_string(),
            ])
            .with_server_pubkey(server_pubkey),
    )
    .await?;

    let client = BitcoinRpcNostrClient.serve(transport).await?;

    let tools = client.list_all_tools().await?;
    println!("Discovered {} tool(s).", tools.len());

    let result = client
        .call_tool(CallToolRequestParams::new("get_blockchain_info"))
        .await?;

    if let Some(content) = result.content.first() {
        if let rmcp::model::RawContent::Text(text) = &content.raw {
            println!("Blockchain info: {}", text.text);
        }
    }

    let result = client
        .call_tool(CallToolRequestParams::new("get_block_count"))
        .await?;

    if let Some(content) = result.content.first() {
        if let rmcp::model::RawContent::Text(text) = &content.raw {
            println!("Block count: {}", text.text);
        }
    }

    let arguments = serde_json::from_value(serde_json::json!({
        "verbosity": 0,
    }))?;

    let result = client
        .call_tool(CallToolRequestParams::new("get_raw_mempool").with_arguments(arguments))
        .await?;

    if let Some(content) = result.content.first() {
        if let rmcp::model::RawContent::Text(text) = &content.raw {
            println!("Raw mempool: {}", text.text);
        }
    }

    client.cancel().await?;
    Ok(())
}
