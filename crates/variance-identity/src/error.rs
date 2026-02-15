use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Failed to create DID: {reason}"))]
    DidCreation { reason: String },

    #[snafu(display("Invalid DID: {did}"))]
    InvalidDid { did: String },

    #[snafu(display("DID not found: {did}"))]
    DidNotFound { did: String },

    #[snafu(display("Username {username} not found"))]
    UsernameNotFound { username: String },

    #[snafu(display("Username {username} already taken"))]
    UsernameTaken { username: String },

    #[snafu(display("IPFS error: {source}"))]
    Ipfs {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("IPNS error: {source}"))]
    Ipns {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Cache error: {message}"))]
    Cache { message: String },

    #[snafu(display("Serialization error: {source}"))]
    Serialization { source: serde_json::Error },

    #[snafu(display("Storage error: {source}"))]
    Storage { source: sled::Error },

    #[snafu(display("Crypto error: {message}"))]
    Crypto { message: String },

    #[snafu(display("P2P error: {source}"))]
    P2p { source: variance_p2p::Error },
}
