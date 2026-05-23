# Contributing to Variance

Thanks for wanting to contribute. This doc covers what you need to get going.

## Community guidelines

Be kind, be professional, focus on the code. Give honest feedback without being a jerk about it. If someone gives you feedback you disagree with, talk it out like adults. The software comes first.

## Getting started

### Prerequisites

- Rust (latest stable)
- Protocol Buffers compiler (`protoc`)
- [just](https://github.com/casey/just) task runner
- Node.js + pnpm (for the frontend)
- Tauri CLI (`cargo install tauri-cli`)

### Building

```bash
# Build the workspace (triggers protobuf codegen)
cargo build

# Check a specific crate
cargo check -p variance-identity

# Run all tests
just test

# Lint and format
just clippy
just fmt

# Run everything (format + clippy + test)
just all
```

### Frontend

```bash
just frontend-install   # once
just frontend-dev       # dev server

# Type check
cd app && pnpm exec tsc --noEmit
```

### Running the app

```bash
# Tauri desktop app (dev mode with hot reload)
just dev

# Two instances for P2P testing
just dev-two

# Headless CLI (debugging)
RUST_LOG=variance=debug cargo run --bin variance -- start
```

### Force protobuf rebuild

```bash
just proto
```

## Project structure

```
crates/
├── variance-proto/      # Protobuf schemas (foundation layer)
├── variance-p2p/        # libp2p networking + protocol handlers
├── variance-identity/   # DID & identity (IPFS/IPNS backed)
├── variance-messaging/  # Chat (Direct DMs + GossipSub groups with MLS)
├── variance-media/      # WebRTC signaling
├── variance-app/        # HTTP API & state (axum)
├── variance-relay/      # Standalone relay server
└── variance-cli/        # Headless CLI for debugging
app/
├── src/                 # React/TypeScript frontend
└── src-tauri/           # Tauri desktop host
```

Dependency flow: `cli -> app -> p2p -> (identity, messaging, media) -> proto`

The Tauri desktop app embeds `variance-app` in-process. The React frontend talks to it over HTTP and WebSocket. No sidecar process.

## How to contribute

### Reporting issues

Check existing issues first. Provide steps to reproduce, system info, and error output. For feature requests, explain the use case.

### Submitting changes

1. Fork and create a feature branch (`git checkout -b feature/your-thing`)
2. Make your changes
3. Test: `just all`
4. Commit with a clear message referencing any related issues
5. Open a pull request describing what and why

### Code style

- `cargo fmt` and `cargo clippy` must pass clean (clippy with `-D warnings`)
- Use `snafu` for error handling in library crates, `anyhow` only in binaries
- Comments should explain "why", not "what"
- No TODOs unless they have a specific reason documented
- Inline constant defaults in `Default` impls (no helper functions for static values)

### Testing

- All new features need tests
- Unit tests for logic, integration tests for behavior across crate boundaries
- Don't test generated code (protobuf output is prost's job)
- Run specific crate tests with `just test-package variance-messaging`

### Adding a new P2P feature

The pattern is:

1. Add a `NodeCommand` variant in `crates/variance-p2p/src/commands.rs`
2. Add an `Event` variant in `crates/variance-p2p/src/events.rs`
3. Handle both in the swarm loop or a protocol handler
4. Subscribe to the event in `EventRouter` and forward it as a `WsMessage`

### Adding a frontend WebSocket event

1. Add the event to `WsEvent` in `app/src/api/types.ts`
2. Handle it in the `switch` in `useWebSocket.ts`
3. Dispatch to the appropriate Zustand store if it affects UI state

## Performance notes

This is a P2P app where latency matters. When contributing:

- Avoid unnecessary allocations in hot paths (message handling, event routing)
- MLS state writes are debounced (500ms); don't bypass the `MlsPersister`
- Profile with [samply](https://github.com/mstange/samply) if you're touching anything perf-sensitive

## Questions?

- Check `docs/` for architecture and protocol docs
- Open an issue for discussion before starting anything large

## License

By contributing, you agree that your contributions will be licensed under the same license as the project.
