// Generated protobuf code — allow lints that prost output can trigger
pub mod identity {
    pub mod v1 {
        #![allow(clippy::large_enum_variant)]
        include!(concat!(env!("OUT_DIR"), "/variance.identity.v1.rs"));
    }
}

pub mod messaging {
    pub mod v1 {
        #![allow(clippy::large_enum_variant)]
        include!(concat!(env!("OUT_DIR"), "/variance.messaging.v1.rs"));
    }
}

pub mod media {
    pub mod v1 {
        #![allow(clippy::large_enum_variant)]
        include!(concat!(env!("OUT_DIR"), "/variance.media.v1.rs"));
    }
}

// Re-exports for convenience
pub use identity::v1 as identity_proto;
pub use media::v1 as media_proto;
pub use messaging::v1 as messaging_proto;
