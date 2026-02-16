use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Failed to create transport: {source}"))]
    Transport {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to listen on {address}: {source}"))]
    Listen {
        address: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to dial {peer_id}: {source}"))]
    Dial {
        peer_id: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Invalid multiaddr: {address}"))]
    InvalidMultiaddr { address: String },

    #[snafu(display("Invalid peer ID: {peer_id}"))]
    InvalidPeerId { peer_id: String },

    #[snafu(display("DHT operation failed: {message}"))]
    Kad { message: String },

    #[snafu(display("GossipSub operation failed: {message}"))]
    Gossipsub { message: String },

    #[snafu(display("Protocol error: {message}"))]
    Protocol { message: String },

    #[snafu(display("Invalid message: {message}"))]
    InvalidMessage { message: String },

    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },
}
