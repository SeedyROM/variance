use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Failed to encrypt message: {message}"))]
    Encryption { message: String },

    #[snafu(display("Failed to decrypt message: {message}"))]
    Decryption { message: String },

    #[snafu(display("Invalid signature for message {message_id}"))]
    InvalidSignature { message_id: String },

    #[snafu(display("Message not found: {message_id}"))]
    MessageNotFound { message_id: String },

    #[snafu(display("Group not found: {group_id}"))]
    GroupNotFound { group_id: String },

    #[snafu(display("Unauthorized: {message}"))]
    Unauthorized { message: String },

    #[snafu(display("Invalid message format: {message}"))]
    InvalidFormat { message: String },

    #[snafu(display("Storage error: {source}"))]
    Storage { source: sled::Error },

    #[snafu(display("Serialization error: {source}"))]
    Serialization { source: serde_json::Error },

    #[snafu(display("Protocol error: {source}"))]
    Protocol { source: prost::DecodeError },

    #[snafu(display("Double Ratchet error: {message}"))]
    DoubleRatchet { message: String },

    #[snafu(display("One-time pre-key was already consumed or invalid: {message}"))]
    StaleOneTimeKey { message: String },

    #[snafu(display("Message expired: {message_id}"))]
    MessageExpired { message_id: String },

    #[snafu(display("Relay storage full"))]
    RelayStorageFull,

    #[snafu(display("Crypto error: {message}"))]
    Crypto { message: String },

    #[snafu(display("MLS group error: {message}"))]
    MlsGroup { message: String },

    #[snafu(display("MLS welcome error: {message}"))]
    MlsWelcome { message: String },

    #[snafu(display("MLS key package error: {message}"))]
    MlsKeyPackage { message: String },

    #[snafu(display("MLS commit error: {message}"))]
    MlsCommit { message: String },

    #[snafu(display("Internal lock poisoned: {message}"))]
    LockPoisoned { message: String },
}
