# Bitcoin RPC over Nostr

Bitcoin RPC over Nostr provides a [Model Context Protocol (MCP)](https://modelcontextprotocol.io) interface to a Bitcoin Core node, using [ContextVM](https://github.com/contextvm) to transport MCP messages over Nostr. Nostr's cryptographic keypairs and signed events provide built-in verification and authorization.

## Running server

```bash
BITCOIN_RPC_URL=http://127.0.0.1:18443 \
BITCOIN_RPC_USER=myuser \
BITCOIN_RPC_PASSWORD=mypass \
cargo run -- server
```

## Running client

```bash
cargo run -- client <server-pub-key-hex>
```

## Architecture

This project bridges two distinct protocol layers:

- **Client ⟷ ContexVM MCP server:** MCP over Nostr.
- **ContexVM MCP server ⟷ bitcoind:** JSON-RPC over HTTP.
