# Variance CLI Usage Guide

Quick reference for using the `variance` command-line tool.

## Installation

```bash
cargo build --release
# Binary will be at: target/release/variance
```

## Quick Start

1. **Generate an identity:**
   ```bash
   variance identity generate
   ```
   This creates `.variance/identity.json` with your DID and signing keys.

2. **Initialize configuration (optional):**
   ```bash
   variance config init
   ```
   This creates `config.toml` with default settings.

3. **Start the node:**
   ```bash
   variance start
   ```
   The node automatically loads your identity from `.variance/identity.json`.

## Commands

### Start Node

Start the Variance node with HTTP API:

```bash
variance start [OPTIONS]
```

**Options:**
- `-c, --config <FILE>` - Path to config file (default: `config.toml`)
- `-l, --listen <ADDR>` - HTTP API address (default: from config or `127.0.0.1:3000`)
- `-d, --did <DID>` - Override DID (optional, for testing only)

The node automatically loads your identity from the path specified in `config.toml` (default: `.variance/identity.json`).

**Examples:**
```bash
# Start with default settings
variance start

# Start with custom listen address
variance start --listen 127.0.0.1:8080

# Start with custom config file
variance start --config my-config.toml
```

**Shutdown:**
Press `Ctrl+C` for graceful shutdown.

### Configuration Management

#### Initialize Config

Create a new configuration file:

```bash
variance config init [OPTIONS]
```

**Options:**
- `-o, --output <FILE>` - Output path (default: `config.toml`)
- `-f, --force` - Overwrite existing file

**Example:**
```bash
variance config init --output my-config.toml
```

#### Show Config

Display current configuration:

```bash
variance config show [OPTIONS]
```

**Options:**
- `-c, --config <FILE>` - Config file to display (default: `config.toml`)

### Identity Management

#### Generate Identity

Create a new DID and signing key:

```bash
variance identity generate [OPTIONS]
```

**Options:**
- `-o, --output <FILE>` - Output file (default: `.variance/identity.json`)
- `-f, --force` - Overwrite existing file

**Examples:**
```bash
# Generate identity at default location (.variance/identity.json)
variance identity generate

# Generate at custom location
variance identity generate --output alice.json
```

**⚠️ Security Note:** The generated file contains your private signing key. Keep it secure!

#### Show Identity

Display identity information:

```bash
variance identity show [OPTIONS]
```

**Options:**
- `-i, --input <FILE>` - Identity file (default: `.variance/identity.json`)

## Configuration File

The `config.toml` file contains settings for:

- **Server**: HTTP API host and port
- **P2P**: libp2p listen addresses and bootstrap peers
- **Identity**: IPFS API endpoint and cache settings
- **Media**: STUN/TURN servers for WebRTC
- **Storage**: Paths for data storage

Example `config.toml`:

```toml
[server]
host = "127.0.0.1"
port = 3000

[p2p]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []

[identity]
ipfs_api = "http://127.0.0.1:5001"
cache_ttl_secs = 3600

[media]
stun_servers = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
]
turn_servers = []

[storage]
base_dir = ".variance"
identity_path = ".variance/identity.json"
identity_cache_dir = ".variance/identity_cache"
message_db_path = ".variance/messages.db"
```

## HTTP API

Once the node is running, the HTTP API is available at the configured address (default: `http://127.0.0.1:3000`).

### Available Endpoints

**Health Check:**
- `GET /health` - Service status

**Calls:**
- `POST /calls/create` - Initiate a call
- `GET /calls/active` - List active calls
- `POST /calls/{id}/accept` - Accept incoming call
- `POST /calls/{id}/reject` - Reject incoming call
- `POST /calls/{id}/end` - End active call

**Signaling:**
- `POST /signaling/offer` - Send WebRTC offer
- `POST /signaling/answer` - Send WebRTC answer
- `POST /signaling/ice` - Send ICE candidate
- `POST /signaling/control` - Send call control message

**Receipts:**
- `POST /receipts/delivered` - Send delivery receipt
- `POST /receipts/read` - Send read receipt
- `GET /receipts/{message_id}` - Get receipts for message

**Typing Indicators:**
- `POST /typing/start` - Start typing
- `POST /typing/stop` - Stop typing
- `GET /typing/{recipient}` - Get typing users

## Environment Variables

Control logging with `RUST_LOG`:

```bash
# Info level (default)
RUST_LOG=variance=info variance start

# Debug level
RUST_LOG=variance=debug variance start

# Trace level
RUST_LOG=variance=trace variance start
```

## Examples

### Development Workflow

```bash
# 1. Generate identity (saves to .variance/identity.json)
variance identity generate

# 2. View identity
variance identity show

# 3. Create config (optional)
variance config init

# 4. Start node (automatically loads identity)
variance start

# 5. Test API (in another terminal)
curl http://localhost:3000/health
```

### Production Deployment

```bash
# 1. Create production identity securely
variance identity generate --output /etc/variance/identity.json

# 2. Set appropriate permissions
chmod 600 /etc/variance/identity.json

# 3. Create production config
variance config init --output /etc/variance/config.toml

# 4. Edit config to set identity_path and production settings
vim /etc/variance/config.toml
# Set: storage.identity_path = "/etc/variance/identity.json"

# 5. Start with production settings
variance start \
  --config /etc/variance/config.toml \
  --listen 0.0.0.0:3000
```

## Troubleshooting

### Port Already in Use

If you get "address already in use", either:
1. Stop the existing process using the port
2. Use a different port: `variance start --did <DID> --listen 127.0.0.1:3001`

### Database Lock Error

If you get "could not acquire lock", ensure no other Variance instance is running with the same database path.

### IPFS Not Running

If identity operations fail, ensure IPFS is running:
```bash
ipfs daemon
```

Or update `config.toml` with the correct IPFS API endpoint.
