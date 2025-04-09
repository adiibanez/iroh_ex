use async_trait::async_trait;
use futures_lite::StreamExt;
use iroh::PublicKey;
use iroh_gossip::net::Event as GossipNetEvent;
use iroh_gossip::{
    net::{Gossip, GossipEvent, GossipSender},
    proto::{Message, TopicId},
};
use rustler::{Env, Error as RustlerError, ResourceArc};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::actor::Actor;
use crate::state::atoms;
use crate::state::ErlangMessageEvent;
use crate::state::NodeRef;
use crate::state::NodeState;
use crate::tokio_runtime::RUNTIME;
use crate::utils::string_to_32_byte_array;

use anyhow::{Context, Result};
use iroh::{
    endpoint::Connection, protocol::ProtocolHandler, Endpoint, NodeAddr, NodeId, SecretKey,
};
use std::error::Error as StdError;
use std::io;

// Define our custom message type
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum CustomMessage {
    AboutMe { from: PublicKey, name: String },
}

// 1. First, let's define structures for topic management
#[derive(Debug)]
struct TopicSubscription {
    sender: GossipSender,
    node_ids: Vec<NodeId>,
    // Could add metadata like subscription time, stats, etc.
}

// 2. Enhanced message types to handle multiple topics
#[derive(Debug)]
pub enum GossipActorMessage {
    Subscribe {
        topic_id: TopicId,
        node_ids: Vec<NodeId>,
    },
    Unsubscribe(TopicId),
    Broadcast {
        topic_id: TopicId,
        data: Vec<u8>,
    },
    HandleEvent {
        topic_id: TopicId,
        event: GossipNetEvent,
    },
    ListTopics,
    Shutdown,
}

// 3. Enhanced Actor with topic management
struct GossipActor {
    gossip: Gossip,
    erlang_sender: mpsc::Sender<ErlangMessageEvent>,
    node_id: PublicKey,
    topics: HashMap<TopicId, TopicSubscription>,
}

impl GossipActor {
    fn new(
        gossip: Gossip,
        erlang_sender: mpsc::Sender<ErlangMessageEvent>,
        node_id: PublicKey,
    ) -> Self {
        Self {
            gossip,
            erlang_sender,
            node_id,
            topics: HashMap::new(),
        }
    }

    async fn handle_subscribe(
        &mut self,
        topic_id: TopicId,
        node_ids: Vec<NodeId>,
    ) -> Result<(), anyhow::Error> {
        // Check if we're already subscribed
        if self.topics.contains_key(&topic_id) {
            tracing::warn!("Already subscribed to topic: {:?}", topic_id);
            return Ok(());
        }

        // Subscribe to the topic
        let topic = self
            .gossip
            .subscribe_and_join(topic_id.clone(), node_ids.clone())
            .await?;

        let (sender, mut receiver) = topic.split();

        // Store subscription info
        self.topics
            .insert(topic_id.clone(), TopicSubscription { sender, node_ids });

        // Spawn a task to handle events for this topic
        let erlang_sender = self.erlang_sender.clone();
        let node_id = self.node_id;
        let topic_id_clone = topic_id.clone();

        tokio::spawn(async move {
            while let Some(event) = receiver.next().await {
                match event {
                    Ok(event) => {
                        // Handle different event types
                        match event {
                            GossipNetEvent::Gossip(GossipEvent::Joined(pub_keys)) => {
                                if let Err(e) = erlang_sender
                                    .send(ErlangMessageEvent {
                                        atom: atoms::iroh_gossip_joined(),
                                        payload: vec![
                                            topic_id_clone.to_string(),
                                            pub_keys
                                                .iter()
                                                .map(|pk| pk.fmt_short())
                                                .collect::<Vec<_>>()
                                                .join(","),
                                        ],
                                    })
                                    .await
                                {
                                    tracing::error!("Failed to send join event: {:?}", e);
                                }
                            }
                            GossipNetEvent::Gossip(GossipEvent::NeighborUp(pub_key)) => {
                                if let Err(e) = erlang_sender
                                    .send(ErlangMessageEvent {
                                        atom: atoms::iroh_gossip_neighbor_up(),
                                        payload: vec![
                                            topic_id_clone.to_string(),
                                            pub_key.fmt_short(),
                                        ],
                                    })
                                    .await
                                {
                                    tracing::error!("Failed to send neighbor up event: {:?}", e);
                                }
                            }
                            GossipNetEvent::Gossip(GossipEvent::NeighborDown(pub_key)) => {
                                if let Err(e) = erlang_sender
                                    .send(ErlangMessageEvent {
                                        atom: atoms::iroh_gossip_neighbor_down(),
                                        payload: vec![
                                            topic_id_clone.to_string(),
                                            pub_key.fmt_short(),
                                        ],
                                    })
                                    .await
                                {
                                    tracing::error!("Failed to send neighbor down event: {:?}", e);
                                }
                            }
                            GossipNetEvent::Gossip(GossipEvent::Received(msg)) => {
                                if let Ok(message) =
                                    serde_json::from_slice::<CustomMessage>(&msg.content)
                                {
                                    match message {
                                        CustomMessage::AboutMe { from, name } => {
                                            if let Err(e) = erlang_sender
                                                .send(ErlangMessageEvent {
                                                    atom: atoms::iroh_gossip_message_received(),
                                                    payload: vec![
                                                        topic_id_clone.to_string(),
                                                        from.fmt_short(),
                                                        name,
                                                    ],
                                                })
                                                .await
                                            {
                                                tracing::error!(
                                                    "Failed to send message event: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            GossipNetEvent::Lagged => {
                                tracing::warn!("Event stream lagged");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Error receiving event for topic {:?}: {:?}",
                            topic_id_clone,
                            e
                        );
                    }
                }
            }
            tracing::info!("Event handler for topic {:?} exiting", topic_id_clone);
        });

        Ok(())
    }

    async fn handle_unsubscribe(&mut self, topic_id: TopicId) -> Result<(), anyhow::Error> {
        if let Some(_subscription) = self.topics.remove(&topic_id) {
            // The subscription will be dropped here, which should clean up resources
            tracing::info!("Unsubscribed from topic: {:?}", topic_id);
        }
        Ok(())
    }

    async fn handle_broadcast(
        &self,
        topic_id: TopicId,
        data: Vec<u8>,
    ) -> Result<(), anyhow::Error> {
        if let Some(subscription) = self.topics.get(&topic_id) {
            let message = CustomMessage::AboutMe {
                from: self.node_id,
                name: String::from_utf8_lossy(&data).to_string(),
            };
            subscription
                .sender
                .broadcast(serde_json::to_vec(&message)?.into())
                .await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Not subscribed to topic: {:?}", topic_id))
        }
    }
}

#[async_trait]
impl Actor for GossipActor {
    type Message = GossipActorMessage;
    type Error = Box<dyn StdError + Send + Sync>;

    async fn handle(&mut self, msg: Self::Message) -> Result<(), Self::Error> {
        match msg {
            GossipActorMessage::Subscribe { topic_id, node_ids } => {
                self.handle_subscribe(topic_id, node_ids)
                    .await
                    .map_err(|e| {
                        Box::new(io::Error::new(io::ErrorKind::Other, e.to_string()))
                            as Box<dyn StdError + Send + Sync>
                    })?;
            }
            GossipActorMessage::Unsubscribe(topic_id) => {
                self.handle_unsubscribe(topic_id).await.map_err(|e| {
                    Box::new(io::Error::new(io::ErrorKind::Other, e.to_string()))
                        as Box<dyn StdError + Send + Sync>
                })?;
            }
            GossipActorMessage::Broadcast { topic_id, data } => {
                self.handle_broadcast(topic_id, data).await.map_err(|e| {
                    Box::new(io::Error::new(io::ErrorKind::Other, e.to_string()))
                        as Box<dyn StdError + Send + Sync>
                })?;
            }
            GossipActorMessage::HandleEvent { topic_id, event } => {
                // Handle the event based on its type
                match event {
                    GossipNetEvent::Gossip(gossip_event) => {
                        // Handle gossip event
                        tracing::debug!(
                            "Handling gossip event for topic {:?}: {:?}",
                            topic_id,
                            gossip_event
                        );
                    }
                    GossipNetEvent::Lagged => {
                        tracing::warn!("Event stream lagged");
                    }
                }
            }
            GossipActorMessage::ListTopics => {
                // Send list of topics back through erlang_sender
                let topics: Vec<String> = self.topics.keys().map(|t| t.to_string()).collect();

                self.erlang_sender
                    .send(ErlangMessageEvent {
                        atom: atoms::iroh_gossip_list_topics(),
                        payload: vec![topics.join(",")],
                    })
                    .await
                    .map_err(|e| {
                        Box::new(io::Error::new(io::ErrorKind::Other, e.to_string()))
                            as Box<dyn StdError + Send + Sync>
                    })?;
            }
            GossipActorMessage::Shutdown => {
                // Clean shutdown of all topics
                for (topic_id, _) in self.topics.drain() {
                    tracing::info!("Shutting down topic: {:?}", topic_id);
                }
            }
        }
        Ok(())
    }
}

// 4. Helper methods for the NodeRef to manage topics
impl NodeRef {
    pub async fn subscribe_to_topic(
        &self,
        topic_id: TopicId,
        node_ids: Vec<NodeId>,
    ) -> Result<(), RustlerError> {
        let state = self.0.lock().unwrap();
        if let Some(actor) = &state.gossip_actor {
            actor
                .send(GossipActorMessage::Subscribe { topic_id, node_ids })
                .await
                .map_err(|e| RustlerError::Term(Box::new(e.to_string())))?;
        }
        Ok(())
    }

    pub async fn unsubscribe_from_topic(&self, topic_id: TopicId) -> Result<(), RustlerError> {
        let state = self.0.lock().unwrap();
        if let Some(actor) = &state.gossip_actor {
            actor
                .send(GossipActorMessage::Unsubscribe(topic_id))
                .await
                .map_err(|e| RustlerError::Term(Box::new(e.to_string())))?;
        }
        Ok(())
    }
}

// 5. Rustler NIF functions to expose topic management
#[rustler::nif(schedule = "DirtyCpu")]
pub fn subscribe_to_topic(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    topic_str: String,
    node_ids: Vec<String>,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let topic_id = TopicId::from_bytes(string_to_32_byte_array(&topic_str));

    // Convert node_ids from strings to NodeIds
    let node_ids = node_ids
        .into_iter()
        .filter_map(|id| {
            let bytes = string_to_32_byte_array(&id);
            PublicKey::from_bytes(&bytes).ok()
        })
        .collect();

    RUNTIME.block_on(async {
        node_ref.subscribe_to_topic(topic_id, node_ids).await?;
        Ok(node_ref)
    })
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn unsubscribe_from_topic(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    topic_str: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let topic_id = TopicId::from_bytes(string_to_32_byte_array(&topic_str));

    RUNTIME.block_on(async {
        node_ref.unsubscribe_from_topic(topic_id).await?;
        Ok(node_ref)
    })
}
