use crate::{behaviour::VarianceBehaviour, config::Config, error::*, events::*, handlers};
use futures::StreamExt;
use libp2p::swarm::SwarmEvent;
use libp2p::{
    gossipsub, identify, kad, mdns, noise, ping, tcp, yamux, PeerId, Swarm, SwarmBuilder,
};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tracing::{debug, info, warn};

pub struct Node {
    swarm: Swarm<VarianceBehaviour>,
    peer_id: PeerId,
    identity_handler: Arc<handlers::identity::IdentityHandler>,
    offline_handler: Arc<handlers::offline::OfflineMessageHandler>,
    signaling_handler: Arc<handlers::signaling::SignalingHandler>,
    events: EventChannels,
}

impl Node {
    pub fn new(config: Config) -> Result<Self> {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        info!("Creating P2P node with peer ID: {}", peer_id);

        // Build Kademlia DHT
        let mut kad_config = kad::Config::default();
        kad_config.set_replication_factor(
            NonZeroUsize::new(config.kad_config.replication_factor)
                .unwrap_or(NonZeroUsize::new(20).unwrap()),
        );
        kad_config.set_provider_record_ttl(Some(Duration::from_secs(
            config.kad_config.provider_record_ttl,
        )));

        let store = kad::store::MemoryStore::new(peer_id);
        let kad = kad::Behaviour::with_config(peer_id, store, kad_config);

        // Build GossipSub
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(
                config.gossipsub_config.heartbeat_interval_secs,
            ))
            .history_length(config.gossipsub_config.history_length)
            .history_gossip(config.gossipsub_config.history_gossip)
            .build()
            .map_err(|e| Error::Gossipsub {
                message: e.to_string(),
            })?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|e| Error::Gossipsub {
            message: e.to_string(),
        })?;

        // Build mDNS
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id).map_err(|e| {
            Error::Transport {
                source: Box::new(e),
            }
        })?;

        // Build Identify
        let identify = identify::Behaviour::new(identify::Config::new(
            "/variance/1.0.0".to_string(),
            keypair.public(),
        ));

        // Build Ping
        let ping = ping::Behaviour::new(ping::Config::new());

        // Build custom protocols
        let identity = crate::protocols::identity::create_identity_behaviour();
        let offline_messages = crate::protocols::messaging::create_offline_message_behaviour();
        let signaling = crate::protocols::media::create_signaling_behaviour();

        // Combine into VarianceBehaviour
        let behaviour = VarianceBehaviour {
            kad,
            gossipsub,
            mdns,
            identify,
            ping,
            identity,
            offline_messages,
            signaling,
        };

        // Build Swarm
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_quic()
            .with_behaviour(|_| behaviour)
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // Initialize protocol handlers
        let identity_handler = Arc::new(handlers::identity::IdentityHandler::new(peer_id));

        let offline_handler = Arc::new(
            handlers::offline::OfflineMessageHandler::with_local_storage(
                peer_id.to_string(),
                &config.storage_path.join("messages"),
            )?,
        );

        let signaling_handler = Arc::new(handlers::signaling::SignalingHandler::new());

        let events = EventChannels::default();

        Ok(Node {
            swarm,
            peer_id,
            identity_handler,
            offline_handler,
            signaling_handler,
            events,
        })
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Get event channels for subscribing to protocol events
    pub fn events(&self) -> &EventChannels {
        &self.events
    }

    pub async fn listen(&mut self, config: &Config) -> Result<()> {
        for addr in &config.listen_addresses {
            self.swarm
                .listen_on(addr.clone())
                .map_err(|e| Error::Listen {
                    address: addr.to_string(),
                    source: Box::new(e),
                })?;
            info!("Listening on {}", addr);
        }

        // Bootstrap DHT with configured peers
        for bootstrap in &config.bootstrap_peers {
            let peer_id: PeerId = bootstrap
                .peer_id
                .parse()
                .map_err(|_| Error::InvalidPeerId {
                    peer_id: bootstrap.peer_id.clone(),
                })?;

            self.swarm
                .behaviour_mut()
                .kad
                .add_address(&peer_id, bootstrap.multiaddr.clone());

            info!(
                "Added bootstrap peer: {} at {}",
                peer_id, bootstrap.multiaddr
            );
        }

        // Bootstrap the DHT
        if !config.bootstrap_peers.is_empty() {
            self.swarm
                .behaviour_mut()
                .kad
                .bootstrap()
                .map_err(|e| Error::Kad {
                    message: format!("Bootstrap failed: {:?}", e),
                })?;
        }

        Ok(())
    }

    pub async fn run(&mut self, mut shutdown: tokio::sync::mpsc::Receiver<()>) -> Result<()> {
        loop {
            select! {
                event = self.swarm.select_next_some() => {
                    self.handle_event(event).await;
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_event(&mut self, event: SwarmEvent<crate::behaviour::VarianceBehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(event) => match event {
                crate::behaviour::VarianceBehaviourEvent::Kad(kad_event) => match kad_event {
                    kad::Event::RoutingUpdated {
                        peer, addresses, ..
                    } => {
                        debug!("Routing updated for {}: {:?}", peer, addresses);
                    }
                    kad::Event::InboundRequest { request } => {
                        debug!("Inbound DHT request: {:?}", request);
                    }
                    _ => {}
                },
                crate::behaviour::VarianceBehaviourEvent::Gossipsub(gossipsub_event) => {
                    if let gossipsub::Event::Message {
                        propagation_source,
                        message_id,
                        message,
                    } = gossipsub_event
                    {
                        debug!(
                            "Got message {} from {} on topic {:?}",
                            message_id, propagation_source, message.topic
                        );
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Mdns(mdns_event) => match mdns_event {
                    mdns::Event::Discovered(peers) => {
                        for (peer_id, multiaddr) in peers {
                            info!("Discovered peer via mDNS: {} at {}", peer_id, multiaddr);
                            self.swarm
                                .behaviour_mut()
                                .kad
                                .add_address(&peer_id, multiaddr);
                        }
                    }
                    mdns::Event::Expired(peers) => {
                        for (peer_id, multiaddr) in peers {
                            debug!("mDNS peer expired: {} at {}", peer_id, multiaddr);
                        }
                    }
                },
                crate::behaviour::VarianceBehaviourEvent::Identify(identify_event) => {
                    if let identify::Event::Received { peer_id, info, .. } = identify_event {
                        debug!("Identified peer {}: {:?}", peer_id, info);
                        for addr in info.listen_addrs {
                            self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                        }
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Ping(ping_event) => match ping_event {
                    ping::Event {
                        peer,
                        result: Ok(rtt),
                        ..
                    } => {
                        debug!("Ping to {} succeeded: {:?}", peer, rtt);
                    }
                    ping::Event {
                        peer,
                        result: Err(e),
                        ..
                    } => {
                        warn!("Ping to {} failed: {}", peer, e);
                    }
                },
                crate::behaviour::VarianceBehaviourEvent::Identity(identity_event) => {
                    use libp2p::request_response::{Event, Message};
                    match identity_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received identity request {:?} from {}: {:?}",
                                    request_id, peer, request
                                );

                                // Send event
                                self.events.send_identity(IdentityEvent::RequestReceived {
                                    peer,
                                    request: request.clone(),
                                });

                                // Handle request and send response
                                let handler = self.identity_handler.clone();
                                match futures::executor::block_on(handler.handle_request(request)) {
                                    Ok(response) => {
                                        debug!("Sending identity response for {:?}", request_id);
                                        let _ = self
                                            .swarm
                                            .behaviour_mut()
                                            .identity
                                            .send_response(channel, response);
                                    }
                                    Err(e) => {
                                        warn!("Failed to handle identity request: {}", e);
                                    }
                                }
                            }
                            Message::Response {
                                request_id,
                                response,
                            } => {
                                debug!(
                                    "Received identity response {:?} from {}: {:?}",
                                    request_id, peer, response
                                );

                                // Send event
                                self.events.send_identity(IdentityEvent::ResponseReceived {
                                    peer,
                                    response: response.clone(),
                                });

                                // Process response (e.g., cache the DID)
                                if let Some(variance_proto::identity_proto::identity_response::Result::Found(
                                    found,
                                )) = response.result
                                {
                                    if let Some(did_doc) = found.did_document {
                                        let handler = self.identity_handler.clone();
                                        let events = self.events.clone();
                                        let did_id = did_doc.id.clone();
                                        tokio::spawn(async move {
                                            if let Ok(did) = variance_identity::did::Did::from_proto(did_doc) {
                                                if handler.cache_did(did).await.is_ok() {
                                                    events.send_identity(IdentityEvent::DidCached { did: did_id });
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        },
                        Event::OutboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "Identity request {:?} to {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::InboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "Identity request {:?} from {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::ResponseSent {
                            peer, request_id, ..
                        } => {
                            debug!("Identity response {:?} sent to {}", request_id, peer);
                        }
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::OfflineMessages(offline_event) => {
                    use libp2p::request_response::{Event, Message};
                    match offline_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received offline message request {:?} from {}: {} messages since {:?}",
                                    request_id, peer, request.limit, request.since_timestamp
                                );

                                // Send event
                                self.events.send_offline_message(
                                    OfflineMessageEvent::FetchRequested {
                                        peer,
                                        did: request.did.clone(),
                                        limit: request.limit,
                                    },
                                );

                                // Handle request and send response
                                let handler = self.offline_handler.clone();
                                match futures::executor::block_on(handler.handle_request(request)) {
                                    Ok(response) => {
                                        debug!(
                                            "Sending {} offline messages in response {:?}",
                                            response.messages.len(),
                                            request_id
                                        );
                                        let _ = self
                                            .swarm
                                            .behaviour_mut()
                                            .offline_messages
                                            .send_response(channel, response);
                                    }
                                    Err(e) => {
                                        warn!("Failed to handle offline message request: {}", e);
                                    }
                                }
                            }
                            Message::Response {
                                request_id,
                                response,
                            } => {
                                debug!(
                                    "Received {} offline messages in response {:?} from {}",
                                    response.messages.len(),
                                    request_id,
                                    peer
                                );

                                // Send event with all received messages
                                self.events.send_offline_message(
                                    OfflineMessageEvent::MessagesReceived {
                                        peer,
                                        messages: response.messages,
                                        has_more: response.has_more,
                                    },
                                );
                            }
                        },
                        Event::OutboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "Offline message request {:?} to {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::InboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "Offline message request {:?} from {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::ResponseSent {
                            peer, request_id, ..
                        } => {
                            debug!("Offline message response {:?} sent to {}", request_id, peer);
                        }
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Signaling(signaling_event) => {
                    use libp2p::request_response::{Event, Message};
                    use variance_proto::media_proto::signaling_message;

                    match signaling_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received WebRTC signaling request {:?} from {} for call {}",
                                    request_id, peer, request.call_id
                                );

                                // Send appropriate event based on message type
                                match &request.message {
                                    Some(signaling_message::Message::Offer(_)) => {
                                        self.events.send_signaling(SignalingEvent::OfferReceived {
                                            peer,
                                            call_id: request.call_id.clone(),
                                            message: request.clone(),
                                        });
                                    }
                                    Some(signaling_message::Message::Answer(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::AnswerReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::IceCandidate(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::IceCandidateReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::Control(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::ControlReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    None => {}
                                }

                                // Handle request and send response
                                let handler = self.signaling_handler.clone();
                                let peer_did = format!("did:peer:{}", peer); // Simplified - should look up actual DID
                                match futures::executor::block_on(
                                    handler.handle_message(peer_did, request),
                                ) {
                                    Ok(response) => {
                                        debug!(
                                            "Sending WebRTC signaling response for {:?}",
                                            request_id
                                        );
                                        let _ = self
                                            .swarm
                                            .behaviour_mut()
                                            .signaling
                                            .send_response(channel, response);
                                    }
                                    Err(e) => {
                                        warn!("Failed to handle signaling request: {}", e);
                                    }
                                }
                            }
                            Message::Response {
                                request_id,
                                response,
                            } => {
                                debug!(
                                    "Received WebRTC signaling response {:?} from {} for call {}",
                                    request_id, peer, response.call_id
                                );

                                // Send appropriate event based on message type
                                match &response.message {
                                    Some(signaling_message::Message::Offer(_)) => {
                                        self.events.send_signaling(SignalingEvent::OfferReceived {
                                            peer,
                                            call_id: response.call_id.clone(),
                                            message: response,
                                        });
                                    }
                                    Some(signaling_message::Message::Answer(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::AnswerReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::IceCandidate(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::IceCandidateReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::Control(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::ControlReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    None => {}
                                }
                            }
                        },
                        Event::OutboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "WebRTC signaling request {:?} to {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::InboundFailure {
                            peer,
                            request_id,
                            error,
                            ..
                        } => {
                            warn!(
                                "WebRTC signaling request {:?} from {} failed: {}",
                                request_id, peer, error
                            );
                        }
                        Event::ResponseSent {
                            peer, request_id, ..
                        } => {
                            debug!(
                                "WebRTC signaling response {:?} sent to {}",
                                request_id, peer
                            );
                        }
                    }
                }
            },
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                info!(
                    "Connection established to {} via {}",
                    peer_id,
                    endpoint.get_remote_address()
                );
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                debug!("Connection to {} closed: {:?}", peer_id, cause);
            }
            SwarmEvent::IncomingConnection { send_back_addr, .. } => {
                debug!("Incoming connection from {}", send_back_addr);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(peer) = peer_id {
                    warn!("Outgoing connection to {} failed: {}", peer, error);
                } else {
                    warn!("Outgoing connection failed: {}", error);
                }
            }
            SwarmEvent::IncomingConnectionError {
                send_back_addr,
                error,
                ..
            } => {
                warn!(
                    "Incoming connection from {} failed: {}",
                    send_back_addr, error
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_node_creation() {
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.storage_path = dir.path().to_path_buf();

        let node = Node::new(config).unwrap();
        assert!(!node.peer_id().to_string().is_empty());
    }

    #[tokio::test]
    async fn test_node_listen() {
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.storage_path = dir.path().to_path_buf();

        let mut node = Node::new(config.clone()).unwrap();
        node.listen(&config).await.unwrap();
    }
}
