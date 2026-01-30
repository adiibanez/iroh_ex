#![no_main]
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
// #![allow(deprecated)]
// #![allow(unused_must_use)]
#![allow(non_local_definitions)]
// #[cfg(not(clippy))]
// #![feature(mpmc_channel)]
#![allow(clippy::too_many_arguments)]

use iroh::address_lookup::{dns::DnsAddressLookup, mdns::MdnsAddressLookup, pkarr::PkarrPublisher};
use iroh::protocol::AcceptError;
use iroh::PublicKey;
use iroh::RelayMap;
use iroh::RelayMode;
use iroh::RelayUrl;
use n0_future::TryFutureExt;
use rustler::types::atom::Atom;
use rustler::NifStruct;
use rustler::OwnedBinary;
use rustler::Binary;
use rustler::{
    Encoder, Env, Error as RustlerError, LocalPid, NifResult, OwnedEnv, ResourceArc, Term,
};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tracing_subscriber::EnvFilter;

use rand::Rng;

use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::env;
use std::future::Future;
use std::sync::{Arc, Mutex};

use std::fmt;
use std::ptr;
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::{
    endpoint::Connection, protocol::ProtocolHandler, Endpoint, EndpointAddr, SecretKey,
};

// NodeId is now just PublicKey in iroh 0.95+
type NodeId = PublicKey;


// use quic_rpc::transport::flume::FlumeConnector;

// pub(crate) type BlobsClient = iroh_blobs::rpc::client::blobs::Client<
//     FlumeConnector<iroh_blobs::rpc::proto::Response, iroh_blobs::rpc::proto::Request>,>;
// pub(crate) type DocsClient = iroh_docs::rpc::client::docs::Client<
//     FlumeConnector<iroh_docs::rpc::proto::Response, iroh_docs::rpc::proto::Request>,>;

use iroh_gossip::{
    api::{Event, GossipReceiver, GossipSender},
    net::Gossip,
    proto::TopicId,
    ALPN as GossipALPN,
};

// Blobs imports
use iroh_blobs::{
    store::mem::MemStore as BlobMemStore,
    BlobsProtocol,
    Hash as BlobHash,
    ALPN as BlobsALPN,
};

// Docs imports
use iroh_docs::{
    protocol::Docs,
    AuthorId,
    NamespaceId,
    ALPN as DocsALPN,
};

// Distributed topic tracker for DHT-based auto-discovery
// TODO: Re-enable when distributed-topic-tracker updates to support iroh-gossip 0.96
// use distributed_topic_tracker::{
//     AutoDiscoveryGossip,
//     RecordPublisher,
//     RecordTopic,
//     signing_keypair,
//     unix_minute,
// };

use iroh::Watcher;

use serde::{Deserialize, Serialize};

use n0_future::boxed::BoxFuture;
use n0_future::StreamExt;

use rand::distr::Alphanumeric;

mod state;
mod tokio_runtime;
mod utils;
mod wrappers;

use crate::state::atoms;
use crate::state::NodeRef;
use crate::state::NodeState;
use crate::state::{ErlangMessageEvent, Payload};
use crate::tokio_runtime::RUNTIME;

// debug dependencies
// use parking_lot::deadlock;
// use tracing_subscriber::{Registry, prelude::*};
// use console_subscriber::ConsoleLayer;

const ALPN: &[u8] = b"iroh-example/echo/0";

static TOPIC_NAME: Lazy<String> = Lazy::new(generate_topic_name);

fn generate_topic_name() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
enum Message {
    AboutMe { from: NodeId, name: String },
    Message { from: NodeId, text: String },
}

impl Message {
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(Into::into)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serde_json::to_vec is infallible")
    }
}

#[rustler::nif]
pub fn generate_secretkey(env: Env) -> Result<String, RustlerError> {
    let _ = env;
    let secret_key = SecretKey::generate(&mut rand::rng());

    let bytes: [u8; 32] = secret_key.to_bytes();
    let hex_string = hex::encode(bytes);
    Ok(hex_string)
}

#[derive(NifStruct)]
#[module = "IrohEx.NodeConfig"]
struct NodeConfig {
    is_whale_node: bool,
    active_view_capacity: u32,
    passive_view_capacity: u32,
    relay_urls: Vec<String>,
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn create_node(
    env: Env,
    pid: LocalPid,
    node_config: NodeConfig,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    // let env_pid_clone = env.pid();
    let monitor_pid = pid;
    let topic_name = TOPIC_NAME.to_string();

    // let relay_url_str = env::var("RELAY_URL").unwrap_or_else(|_| "http://localhost:3340".to_string());

    // let relay_url = RelayUrl::from_str(&relay_url_str)
    //     .expect("Failed to parse relay url from environment or default");
    // let relay_map = RelayMap::from_url(relay_url);

    // // let relay_url_str = "https://euw1-1.relay.iroh.network./";
    // let relay_url_str = "http://localhost:3340";

    // let relay_url = RelayUrl::from_str(relay_url_str).expect("Failed to parse relay url");
    // let relay_map = RelayMap::from_url(relay_url);

    let relay_mode = if let Ok(url_str) = env::var("RELAY_URL") {
        let relay_url = RelayUrl::from_str(&url_str).expect("Failed to parse RELAY_URL");
        let relay_map = RelayMap::from(relay_url);
        RelayMode::Custom(relay_map)
    } else if env::var("RELAY_EU_ONLY").is_ok() {
        let relay_url = RelayUrl::from_str("https://euw1-1.relay.iroh.network./")
            .expect("Failed to parse hardcoded EU relay URL");
        let relay_map = RelayMap::from(relay_url);
        RelayMode::Custom(relay_map)
    } else if node_config.relay_urls.len() > 0 {
        let relay_url =
            RelayUrl::from_str(&node_config.relay_urls[0]).expect("Failed to parse relay url");
        let relay_map = RelayMap::from(relay_url);
        RelayMode::Custom(relay_map)
    } else if env::var("RELAY_DISABLED").is_ok() {
        RelayMode::Disabled
    } else {
        RelayMode::Default
    };

    tracing::trace!("RELAY config {:?}", relay_mode);

    let endpoint_builder = Endpoint::builder()
        .relay_mode(relay_mode)
        .address_lookup(PkarrPublisher::n0_dns())
        .address_lookup(DnsAddressLookup::n0_dns())
        .address_lookup(MdnsAddressLookup::builder());

    let hyparview_config = if node_config.is_whale_node {
        iroh_gossip::proto::HyparviewConfig {
            active_view_capacity: node_config.active_view_capacity as usize,
            passive_view_capacity: node_config.passive_view_capacity as usize,
            shuffle_interval: Duration::from_secs(5),
            ..Default::default()
        }
    } else {
        let mut hc = iroh_gossip::proto::HyparviewConfig::default();
        hc.shuffle_interval = Duration::from_secs(5);
        hc
    };

    let gossip_builder = Gossip::builder().membership_config(hyparview_config);

    let (resource, _monitor_ref) = RUNTIME.block_on(async move {

        let endpoint: Endpoint = endpoint_builder.bind()
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Endpoint error: {}", e))))?;

        let endpoint_clone = endpoint.clone();
        let router_builder = iroh::protocol::Router::builder(endpoint.clone());

        let gossip = gossip_builder
            .spawn(endpoint.clone());

        // Initialize blobs store
        let blobs_store = BlobMemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blobs_store, None);

        // Initialize docs (requires blobs and gossip)
        let docs = Docs::memory()
            .spawn(endpoint.clone(), blobs_store.clone().into(), gossip.clone())
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Docs error: {}", e))))?;

        let router = router_builder
                            .accept(GossipALPN, gossip.clone())
                            .accept(BlobsALPN, blobs_protocol)
                            .accept(DocsALPN, docs.clone())
                            .accept(ALPN, Echo)
                            .spawn();

        let router_clone = router.clone();

        let node_ids = vec![];
        let topic = gossip
            .subscribe(
                TopicId::from_bytes(utils::string_to_32_byte_array(&topic_name)),
                node_ids,
            ).await.unwrap();
            // .await
            // .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

        let (mpsc_event_sender, mpsc_event_receiver) = mpsc::channel::<ErlangMessageEvent>(1000);
        let (sender, receiver) = topic.split();
        let mpsc_event_receiver_arc = Arc::new(RwLock::new(mpsc_event_receiver));

        let state = NodeState::new(
            monitor_pid,
            endpoint_clone.clone(),
            router_clone.clone(),
            gossip.clone(),
            sender,
            receiver,
            mpsc_event_sender,
            mpsc_event_receiver_arc.clone(),
            Some(blobs_store),
            Some(docs),
        );

        let resource = ResourceArc::new(NodeRef(Arc::new(Mutex::new(state))));
        let monitor_ref = env.monitor(&resource, &monitor_pid);

        // Start task inside async
        let node_addr_short = endpoint.id().fmt_short().to_string();
        let handler_pid = monitor_pid;

        // let handler_monitor = monitor_ref;

        let erlang_event_handler_task = Some(tokio::spawn(async move {
            let mut mpsc_event_receiver = mpsc_event_receiver_arc.write().await;
            let mut msg_env = OwnedEnv::new();

            while let Some(event) = mpsc_event_receiver.recv().await {
                if let Err(e) = msg_env.send_and_clear(&handler_pid, |env| {
                    let terms: Vec<Term> = match event.payload {
                        Payload::String(s) => vec![s.encode(env)],
                        Payload::Binary(b) => vec![b.encode(env)],
                        Payload::Tuple(t) => t.iter().map(|p| p.encode(env)).collect(),
                        Payload::Map(m) => {
                            let mut terms = Vec::new();
                            for (k, v) in m {
                                terms.push(k.encode(env));
                                terms.push(v.encode(env));
                            }
                            terms
                        },
                        Payload::List(l) => l.iter().map(|p| p.encode(env)).collect(),
                        Payload::Integer(i) => vec![i.encode(env)],
                        Payload::Float(f) => vec![f.encode(env)],
                    };

                    match terms.len() {
                        0 => event.atom.encode(env),
                        1 => (event.atom, terms[0]).encode(env),
                        2 => (event.atom, terms[0], terms[1]).encode(env),
                        3 => (event.atom, terms[0], terms[1], terms[2]).encode(env),
                        4 => (event.atom, terms[0], terms[1], terms[2], terms[3]).encode(env),
                        5 => (event.atom, terms[0], terms[1], terms[2], terms[3], terms[4]).encode(env),
                        _ => (event.atom, terms.to_vec()).encode(env),
                    }
                }) {
                    tracing::warn!(
                        "⚠️ erlang_msg_event_handler Failed to send erlang message node:{:?}, atom:{:?}, err:{:?}, pid:{:?}",
                        node_addr_short,
                        event.atom,
                        e,
                        handler_pid.as_c_arg(),
                    );
                }
            }
        }));

        {
            let mut state = resource.0.lock().unwrap();
            state.erlang_event_handler_task = erlang_event_handler_task;
        }

        Ok::<_, RustlerError>((resource, monitor_ref))
        // --- End of async logic ---
    })?;

    Ok(resource)
}
// Arc::new(RwLock::new(
// async fn erlang_msg_event_handler(
//     receiver_arc: Arc<RwLock<mpsc::Receiver<ErlangMessageEvent>>>,
//     pid: LocalPid,
//     monitor_ref: Option<Monitor>
// ) {
//     // async fn erlang_msg_event_handler(mut receiver: mpsc::Receiver<ErlangMessageEvent>, pid: LocalPid) {
//     let mut receiver = receiver_arc.write().await;
//     let mut msg_env = OwnedEnv::new();

//     while let Some(event) = receiver.recv().await {

//         if let Err(e) = msg_env.send_and_clear(&pid, |env| {
//             let terms: Vec<Term> = event.payload.iter().map(|s| s.encode(env)).collect();

//             match terms.len() {
//                 0 => event.atom.encode(env),
//                 1 => (event.atom, terms[0]).encode(env),
//                 2 => (event.atom, terms[0], terms[1]).encode(env),
//                 3 => (event.atom, terms[0], terms[1], terms[2]).encode(env),
//                 _ => (event.atom, terms.to_vec()).encode(env),
//             }
//         }) {
//             tracing::warn!(
//                 "⚠️ erlang_msg_event_handler Failed to send erlang message: {:?} {:?}",
//                 e,
//                 pid.as_c_arg()
//             );
//         }
//     }
// }

#[rustler::nif(schedule = "DirtyCpu")]
pub fn create_ticket(env: Env, node_ref: ResourceArc<NodeRef>) -> Result<String, RustlerError> {
    let _ = env;
    println!("Create ticket");

    let resource_arc = node_ref.0.clone();

    let (endpoint, _gossip): (Endpoint, Gossip) = {
        let state = resource_arc.lock().unwrap();
        (state.endpoint.clone(), state.gossip.clone())
    };

    let topic = TopicId::from_bytes(utils::string_to_32_byte_array(&TOPIC_NAME.to_string()));

    // Get our address information
    let endpoint_addr = endpoint.addr();

    let ticket = Ticket { topic, endpoint_addr };

    Ok(ticket.to_string())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn gen_node_addr(node_ref: ResourceArc<NodeRef>) -> NifResult<String> {
    let resource_arc = node_ref.0.clone();
    let endpoint: Endpoint = {
        let state = resource_arc.lock().unwrap();
        state.endpoint.clone()
    };

    let node_id = endpoint.id();
    Ok(node_id.fmt_short().to_string())
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn send_message(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    message: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let _ = env;
    let resource_arc = node_ref.0.clone();

    let (endpoint, _gossip, sender) = {
        let state = resource_arc.lock().unwrap();
        (
            state.endpoint.clone(),
            state.gossip.clone(),
            state.sender.clone(),
        )
    };

    let message = Message::AboutMe {
        from: endpoint.id(),
        name: message,
    };

    let result = RUNTIME.block_on(sender.broadcast(message.to_vec().into()));
    if let Err(e) = result {
        tracing::error!("Failed to send message: {:?}", e);
        return Err(RustlerError::Term(Box::new(e.to_string())));
    }

    Ok(node_ref)
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn connect_node(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    ticket: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let node_ref_clone = node_ref.clone();

    let resource_arc = node_ref.0.clone();

    let (pid, endpoint_clone) = {
        let state = resource_arc.lock().unwrap();
        (state.pid, state.endpoint.clone())
    };

    let pid_clone = env.pid();

    RUNTIME.spawn(async move {
        if let Err(e) = connect_node_async_internal(node_ref_clone, pid_clone, ticket).await {
            tracing::error!("❌ Error in async task: {:?}", e);
        }
    });

    // Return immediately, allowing Elixir's Task to execute in parallel
    Ok(node_ref)
}

async fn connect_node_async_internal(
    node_ref: ResourceArc<NodeRef>,
    pid: LocalPid,
    ticket: String,
) -> Result<()> {
    let resource_arc = node_ref.0.clone();

    let _msg_env = OwnedEnv::new();

    let Ticket { topic, endpoint_addr } = Ticket::from_str(&ticket).context("❌ Failed to parse ticket")?;

    let (endpoint, gossip, node_id, node_id_short, erlang_sender_clone) = {
        let state = resource_arc.lock().unwrap();
        (
            state.endpoint.clone() as Endpoint,
            state.gossip.clone() as Gossip,
            state.endpoint.id() as PublicKey,
            state.endpoint.id().fmt_short().to_string(),
            state.mpsc_event_sender.clone(),
        )
    };

    let endpoint_clone = endpoint.clone();
    let node_ref_clone = node_ref.clone();

    tracing::debug!(
        "connect_node endpoint_Ptr:{:?} topic: {:?} endpoint_addr: {:?}",
        &endpoint as *const _,
        topic,
        endpoint_addr
    );

    // Get the remote node_id from the ticket's endpoint_addr
    let remote_node_id = endpoint_addr.id;

    // Skip if trying to connect to ourselves
    let node_ids: Vec<PublicKey> = if remote_node_id != node_id {
        vec![remote_node_id]
    } else {
        tracing::debug!("Skipping self-connection");
        vec![]
    };

    // let home_relay_watcher = endpoint_clone.home_relay();

    // tokio::time::sleep(Duration::from_millis(10)).await;

    // RUNTIME.block_on(async {
    //     tokio::time::sleep(Duration::from_millis(10)).await;
    // });

    /*let home_relay = RUNTIME
            .block_on(home_relay_watcher.clone().initialized())
            .map_err(|e| RustlerError::Term(Box::new(format!("Home relay error: {}", e))));

        tracing::debug!("Home relay {:?}", home_relay);
    */
    /*match home_relay_watcher.initialized().await {
        Ok(home_relay) => tracing::debug!("Homerelay: Ok {:?}", home_relay),
        Err(error) => tracing::error!("Homerelay: NOK {:?}", error)
    }*/

    // let relay_url_test: RelayUrl = endpoint_clone.home_relay().initialized().await?;
    // let relay_url = String::from_str("test").unwrap();

    // let relay_url = endpoint_clone.home_relay().initialized().await?;
    // let relay_url = endpoint_clone.home_relay().get().iter();

    // tracing::debug!("Watcher RelayUrl: {:?}", relay_url);

    if let Err(e) = erlang_sender_clone
        .send(ErlangMessageEvent {
            atom: atoms::iroh_node_connected(),
            payload: Payload::List(vec![
                Payload::String(node_id.fmt_short().to_string()),
            ]),
        })
        .await
    {
        tracing::warn!(
            "❌ GossipEvent::Joined Failed to send erlang message: {:?}",
            e
        );
    }

    // if let Err(e) = erlang_sender_clone
    //     .send(ErlangMessageEvent {
    //         atom: atoms::iroh_node_test(),
    //         payload: Payload::List(vec![
    //             Payload::String("Outer msg".to_string()),
    //             // Payload::String(relay_url.as_str().to_string()),
    //         ]),
    //     })
    //     .await
    // {
    //     tracing::warn!(
    //         "❌ GossipEvent::Joined Failed to send erlang message: {:?}",
    //         e
    //     );
    // }

    let pid_clone = pid;

    let erlang_sender_clone_inner = erlang_sender_clone.clone();
    let event_handler_task = Some(RUNTIME.spawn(async move {

        // if let Err(e) = erlang_sender_clone_inner
        //     .send(ErlangMessageEvent {
        //         atom: atoms::iroh_node_test(),
        //         payload: Payload::List(vec![
        //             Payload::String("Inner msg".to_string()),
        //             // Payload::String(relay_url.as_str().to_string()),
        //         ]),
        //     })
        //     .await
        // {
        //     tracing::warn!(
        //         "❌ GossipEvent::Joined Failed to send erlang message: {:?}",
        //         e
        //     );
        // }

        let topic = gossip
            .subscribe_and_join(topic, node_ids)
            .await
            .context("❌ Failed to subscribe and join gossip")
            .unwrap();

        tracing::debug!("Subscribed to: {:?}", topic);

        let pid_clone = pid;
        // let erlang_sender_clone = erlang_sender_clone.clone();
        let node_id_short_clone = node_id_short.clone();

        let (sender, mut receiver): (GossipSender, GossipReceiver) = topic.split();

        {
            let mut state = node_ref_clone.0.lock().unwrap();
            state.sender = sender.clone();
        }

        /*// ⬇️ Debug, Keep sender alive!
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
        });*/

        while let Some(event) = receiver.next().await {
            match event {
                Ok(event) => {
                    // tracing::debug!("Event {:?}", event);

                    match event {

                        Event::NeighborUp(pub_key) => {

                            use n0_future::StreamExt;

                            // let neighbor_count = receiver.neighbors().count();
                            let neighbor_count = receiver.neighbors().count();

                            // tracing::debug!("NeighborUp {:?} {:?}", pub_key, neighbor_count);

                            if erlang_sender_clone_inner.is_closed() {
                                tracing::error!("❌ GossipEvent::NeighborUp: erlang_sender_clone is closed");
                                continue;
                            }

                            let event = ErlangMessageEvent {
                                atom: atoms::iroh_gossip_neighbor_up(),
                                payload: Payload::List(vec![
                                    Payload::String(node_id_short_clone.clone()),
                                    Payload::String(pub_key.fmt_short().to_string()),
                                    Payload::Integer(neighbor_count as i64),
                                ]),
                            };

                            match erlang_sender_clone_inner.send(event).await {
                                Ok(_) => {
                                    tracing::trace!("✅ NeighborUp event sent successfully");
                                }
                                Err(e) => {
                                    tracing::error!("❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}", e);
                                }
                            }
                        }

                        Event::NeighborDown(pub_key) => {
                            if erlang_sender_clone_inner.is_closed() {
                                tracing::error!("❌ GossipEvent::NeighborDown: erlang_sender_clone is closed");
                                continue;
                            }

                            let event = ErlangMessageEvent {
                                atom: atoms::iroh_gossip_neighbor_down(),
                                payload: Payload::List(vec![
                                    Payload::String(node_id_short_clone.clone()),
                                    Payload::String(pub_key.fmt_short().to_string()),
                                ]),
                            };

                            match erlang_sender_clone_inner.send(event).await {
                                Ok(_) => {
                                    tracing::trace!("✅ NeighborDown event sent successfully");
                                }
                                Err(e) => {
                                    tracing::error!("❌ GossipEvent::NeighborDown Failed to send erlang message: {:?}", e);
                                }
                            }
                        }

                        Event::Received(msg) => {
                            // tracing::debug!("Received message: {:?}", msg);

                            if erlang_sender_clone_inner.is_closed() {
                                tracing::error!("❌ GossipEvent::Received: erlang_sender_clone is closed");
                                continue;
                            }

                            match Message::from_bytes(&msg.content) {
                                Ok(message) => match message {
                                    Message::AboutMe { from, name } => {
                                        tracing::debug!("FROM: {} MSG: {}", from.fmt_short(), name);

                                        let event = ErlangMessageEvent {
                                            atom: atoms::iroh_gossip_message_received(),
                                            payload: Payload::List(vec![
                                                Payload::String(node_id_short_clone.clone()),
                                                Payload::String(name.clone()),
                                            ]),
                                        };

                                        match erlang_sender_clone_inner.send(event).await {
                                            Ok(_) => {
                                                tracing::debug!("✅ Message received event sent successfully");
                                            }
                                            Err(e) => {
                                                tracing::error!("❌ GossipEvent::Received Failed to send erlang message: {:?}", e);
                                            }
                                        }
                                    }
                                    Message::Message { from, text } => {
                                        tracing::debug!("📝 {}: {}", from, text);
                                    }
                                },
                                Err(e) => {
                                    tracing::warn!("❌ GossipEvent::Received Failed to parse message: {:?}", e);
                                }
                            }
                        }
                        unhandled_event => {
                            tracing::debug!("🔍 Ignored unhandled event: {:?}", unhandled_event);
                            let message = format!("🔍 Ignored unhandled event: {:?}", unhandled_event);

                            if let Err(e) = erlang_sender_clone_inner
                                .send(ErlangMessageEvent {
                                    atom: atoms::iroh_gossip_message_unhandled(),
                                    payload: Payload::List(vec![
                                        Payload::String(message),
                                    ]),
                                })
                                .await
                            {
                                tracing::warn!("❌ unhandled_event {:?} Failed to send erlang message: {:?}", unhandled_event, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("❌ Failed to receive event: {:?}", e);
                }
            }
        }
        // tracing::info!("Event handler exiting");
        // });
    }));

    {
        let mut state = node_ref.0.lock().unwrap();
        state.event_handler_task = event_handler_task;
    }

    // {
    //     let state = node_ref.0.lock().unwrap();
    //     tracing::debug!("Event_handler {:?}", state.event_handler_task);
    // }

    Ok(())
}

use std::collections::HashMap;

#[rustler::nif(schedule = "DirtyCpu")]
fn disconnect_node(node_ref: ResourceArc<NodeRef>) -> NifResult<()> {
    let node = node_ref.0.clone();

    let endpoint = {
        let state = node_ref.0.lock().unwrap();
        state.endpoint.clone() as Endpoint
    };

    RUNTIME.spawn(async move {
        endpoint.close().await;
    });

    // let node = node_ref.lock().unwrap();
    // node.disconnect_all();  // Assuming an API to disconnect all peers
    Ok(())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn list_peers(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let _node = node_ref.0.clone();

    let endpoint = {
        let state = node_ref.0.lock().unwrap();
        state.endpoint.clone() as Endpoint
    };

    // In iroh 0.95+, remote_info_iter was removed
    // We return the endpoint's own id as a placeholder
    // In practice, peer tracking should be done via gossip neighbors
    let peers: Vec<String> = vec![
        format!("self:{}", endpoint.id().fmt_short())
    ];

    Ok(peers)
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn cleanup(env: Env, node_ref: ResourceArc<NodeRef>) -> NifResult<()> {
    // Get the monitor ref before dropping
    let monitor_ref = {
        let state = node_ref.0.lock().unwrap();
        state.monitor_ref
    };

    // Demonitor if needed
    if let Some(ref monitor) = monitor_ref {
        env.demonitor(&node_ref, monitor);
    }

    // Drop the ResourceArc which will trigger NodeState::drop
    drop(node_ref);

    // Give the runtime a chance to complete cleanup
    RUNTIME.block_on(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    Ok(())
}

// The protocol definition:
#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    fn accept(
        &self,
        connection: Connection,
    ) -> impl Future<Output = Result<(), AcceptError>> + Send {
        Box::pin(async move {
            let (mut send, mut recv) = connection.accept_bi().await.map_err(AcceptError::from)?;

            let _bytes_sent = tokio::io::copy(&mut recv, &mut send)
                .await
                .map_err(AcceptError::from)?;

            send.finish().map_err(AcceptError::from)?;
            connection.closed().await;

            Ok(())
        })
    }
}

// ============================================================================
// BLOB NIF FUNCTIONS
// ============================================================================

/// Add a blob from binary data, returns the hash as hex string
#[rustler::nif(schedule = "DirtyCpu")]
pub fn blob_add(node_ref: ResourceArc<NodeRef>, data: Binary) -> NifResult<String> {
    let blobs_store = {
        let state = node_ref.0.lock().unwrap();
        state.blobs_store.clone()
    };

    let blobs_store = blobs_store.ok_or_else(|| {
        rustler::Error::Term(Box::new("Blobs store not initialized"))
    })?;

    let hash = RUNTIME.block_on(async {
        let tag_info = blobs_store.add_slice(data.as_slice()).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Blob add error: {}", e))))?;
        Ok::<_, rustler::Error>(tag_info.hash.to_string())
    })?;

    Ok(hash)
}

/// Get a blob by hash, returns the binary data
#[rustler::nif(schedule = "DirtyCpu")]
pub fn blob_get<'a>(env: Env<'a>, node_ref: ResourceArc<NodeRef>, hash_str: String) -> NifResult<Binary<'a>> {
    let blobs_store = {
        let state = node_ref.0.lock().unwrap();
        state.blobs_store.clone()
    };

    let blobs_store = blobs_store.ok_or_else(|| {
        rustler::Error::Term(Box::new("Blobs store not initialized"))
    })?;

    let hash = hash_str.parse::<BlobHash>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid hash: {}", e))))?;

    let data = RUNTIME.block_on(async {
        let bytes = blobs_store.get_bytes(hash).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Blob get error: {}", e))))?;

        Ok::<_, rustler::Error>(bytes.to_vec())
    })?;

    let mut binary = OwnedBinary::new(data.len()).unwrap();
    binary.as_mut_slice().copy_from_slice(&data);
    Ok(binary.release(env))
}

/// List all blob hashes in the store
#[rustler::nif(schedule = "DirtyCpu")]
pub fn blob_list(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let blobs_store = {
        let state = node_ref.0.lock().unwrap();
        state.blobs_store.clone()
    };

    let blobs_store = blobs_store.ok_or_else(|| {
        rustler::Error::Term(Box::new("Blobs store not initialized"))
    })?;

    let hashes = RUNTIME.block_on(async {
        let progress = blobs_store.list();
        let hash_list = progress.hashes().await
            .map_err(|e| rustler::Error::Term(Box::new(format!("List error: {}", e))))?;
        Ok::<_, rustler::Error>(hash_list.into_iter().map(|h| h.to_string()).collect())
    })?;

    Ok(hashes)
}

// ============================================================================
// DOCS NIF FUNCTIONS
// ============================================================================

/// Create a new author for documents
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_create_author(node_ref: ResourceArc<NodeRef>) -> NifResult<String> {
    let docs = {
        let state = node_ref.0.lock().unwrap();
        state.docs.clone()
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let author_id = RUNTIME.block_on(async {
        let author = docs.author_create().await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Create author error: {}", e))))?;
        Ok::<_, rustler::Error>(author.to_string())
    })?;

    Ok(author_id)
}

/// Create a new document, returns namespace_id
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_create(node_ref: ResourceArc<NodeRef>) -> NifResult<String> {
    let docs = {
        let state = node_ref.0.lock().unwrap();
        state.docs.clone()
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let namespace_id = RUNTIME.block_on(async {
        let doc = docs.create().await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Create doc error: {}", e))))?;
        Ok::<_, rustler::Error>(doc.id().to_string())
    })?;

    Ok(namespace_id)
}

/// Set an entry in a document
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_set_entry(
    node_ref: ResourceArc<NodeRef>,
    namespace_id_str: String,
    author_id_str: String,
    key: String,
    value: Binary,
) -> NifResult<String> {
    let docs = {
        let state = node_ref.0.lock().unwrap();
        state.docs.clone()
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let namespace_id = namespace_id_str.parse::<NamespaceId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid namespace: {}", e))))?;

    let author_id = author_id_str.parse::<AuthorId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid author: {}", e))))?;

    let hash = RUNTIME.block_on(async {
        let doc = docs.open(namespace_id).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Open doc error: {}", e))))?
            .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

        let hash = doc.set_bytes(author_id, key.as_bytes().to_vec(), value.as_slice().to_vec()).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Set entry error: {}", e))))?;

        Ok::<_, rustler::Error>(hash.to_string())
    })?;

    Ok(hash)
}

/// Get an entry from a document - returns content hash (use blob_get to retrieve content)
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_get_entry(
    node_ref: ResourceArc<NodeRef>,
    namespace_id_str: String,
    author_id_str: String,
    key: String,
) -> NifResult<String> {
    let (docs, blobs_store) = {
        let state = node_ref.0.lock().unwrap();
        (state.docs.clone(), state.blobs_store.clone())
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let blobs_store = blobs_store.ok_or_else(|| {
        rustler::Error::Term(Box::new("Blobs store not initialized"))
    })?;

    let namespace_id = namespace_id_str.parse::<NamespaceId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid namespace: {}", e))))?;

    let author_id = author_id_str.parse::<AuthorId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid author: {}", e))))?;

    let content_hash = RUNTIME.block_on(async {
        let doc = docs.open(namespace_id).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Open doc error: {}", e))))?
            .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

        let entry = doc.get_exact(author_id, key.as_bytes(), false).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Get entry error: {}", e))))?
            .ok_or_else(|| rustler::Error::Term(Box::new("Entry not found")))?;

        Ok::<_, rustler::Error>(entry.content_hash().to_string())
    })?;

    Ok(content_hash)
}

/// Get an entry value directly from a document
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_get_entry_value<'a>(
    env: Env<'a>,
    node_ref: ResourceArc<NodeRef>,
    namespace_id_str: String,
    author_id_str: String,
    key: String,
) -> NifResult<Binary<'a>> {
    let (docs, blobs_store) = {
        let state = node_ref.0.lock().unwrap();
        (state.docs.clone(), state.blobs_store.clone())
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let blobs_store = blobs_store.ok_or_else(|| {
        rustler::Error::Term(Box::new("Blobs store not initialized"))
    })?;

    let namespace_id = namespace_id_str.parse::<NamespaceId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid namespace: {}", e))))?;

    let author_id = author_id_str.parse::<AuthorId>()
        .map_err(|e| rustler::Error::Term(Box::new(format!("Invalid author: {}", e))))?;

    let data = RUNTIME.block_on(async {
        let doc = docs.open(namespace_id).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Open doc error: {}", e))))?
            .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

        let entry = doc.get_exact(author_id, key.as_bytes(), false).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Get entry error: {}", e))))?
            .ok_or_else(|| rustler::Error::Term(Box::new("Entry not found")))?;

        // Get content via blobs store using the content hash
        let content_hash: BlobHash = entry.content_hash().into();
        let bytes = blobs_store.get_bytes(content_hash).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Get content error: {}", e))))?;

        Ok::<_, rustler::Error>(bytes.to_vec())
    })?;

    let mut binary = OwnedBinary::new(data.len()).unwrap();
    binary.as_mut_slice().copy_from_slice(&data);
    Ok(binary.release(env))
}

/// List all documents
#[rustler::nif(schedule = "DirtyCpu")]
pub fn docs_list(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let docs = {
        let state = node_ref.0.lock().unwrap();
        state.docs.clone()
    };

    let docs = docs.ok_or_else(|| {
        rustler::Error::Term(Box::new("Docs not initialized"))
    })?;

    let namespaces = RUNTIME.block_on(async {
        let mut namespaces = Vec::new();
        // docs.list() returns a Future that resolves to a Stream
        let mut stream = docs.list().await
            .map_err(|e| rustler::Error::Term(Box::new(format!("List error: {}", e))))?;
        while let Some(result) = stream.next().await {
            match result {
                Ok((namespace, _)) => namespaces.push(namespace.to_string()),
                Err(e) => tracing::warn!("Error listing doc: {}", e),
            }
        }
        Ok::<_, rustler::Error>(namespaces)
    })?;

    Ok(namespaces)
}

// ============================================================================
// DHT AUTO-DISCOVERY FUNCTIONS
// ============================================================================
// TODO: Re-enable when distributed-topic-tracker updates to support iroh-gossip 0.96
//
// /// Subscribe to a topic with DHT-based auto-discovery
// /// This allows nodes to find each other without exchanging tickets
// #[rustler::nif(schedule = "DirtyCpu")]
// pub fn subscribe_with_auto_discovery(
//     env: Env,
//     node_ref: ResourceArc<NodeRef>,
//     topic_name: String,
//     secret_seed: Option<String>,
// ) -> Result<ResourceArc<NodeRef>, RustlerError> {
//     let node_ref_clone = node_ref.clone();
//     let pid = env.pid();
//
//     RUNTIME.spawn(async move {
//         if let Err(e) = subscribe_with_auto_discovery_internal(node_ref_clone, pid, topic_name, secret_seed).await {
//             tracing::error!("❌ Error in auto-discovery subscription: {:?}", e);
//         }
//     });
//
//     Ok(node_ref)
// }
//
// async fn subscribe_with_auto_discovery_internal(
//     node_ref: ResourceArc<NodeRef>,
//     pid: LocalPid,
//     topic_name: String,
//     secret_seed: Option<String>,
// ) -> Result<()> {
//     ... (function body commented out - see git history for full code)
// }

// // async fn log_discovery_stream(endpoint: Endpoint, pid: LocalPid) {
// async fn log_discovery_stream(node_ref: ResourceArc<NodeRef>, pid: LocalPid) {
//     let (endpoint, erlang_sender_clone, mut stream) = {
//         let state = node_ref.0.lock().unwrap(); // Acquire lock
//         (
//             state.endpoint.clone() as Endpoint,
//             state.mpsc_event_sender.clone() as tokio::sync::mpsc::Sender<ErlangMessageEvent>,
//             state.endpoint.discovery_stream(), // as Stream<Item = Result<DiscoveryItem, Lagged>>
//         )
//     };

//     // let endpoint = state_clone.endpoint.clone();
//     // let erlang_sender_clone = state_clone.mpsc_event_sender.clone();

//     // let mut stream = endpoint.discovery_stream();
//     let msg_env = OwnedEnv::new();

//     while let Some(result) = stream.next().await {
//         match result {
//             Ok(discovery_item) => {
//                 let node_addr: NodeAddr = (discovery_item as DiscoveryItem).into_node_addr();

//                 // let remote_info: RemoteInfo = endpoint.remote_info_iter()
//                 //     .into(Vec)
//                 //     // .find(|n| n.node_id == node_addr.node_id)
//                 //     .expect("Expected at least one RemoteInfo");

//                 let remote_info_vec: Vec<RemoteInfo> = endpoint
//                     .remote_info_iter()
//                     .filter(|n| n.node_id != node_addr.node_id)
//                     .collect::<Vec<_>>();

//                 for info in &remote_info_vec {
//                     tracing::info!("{}", format_remote_info(info));
//                 }

//                 // tracing::info!(
//                 //     "🔍 {:?} Discovered Node: {:?}",
//                 //     endpoint.node_id().fmt_short(),
//                 //     node_addr,
//                 //     // remote_info_vec
//                 //     //     .iter()
//                 //     //     .map(|info| format!("{:?}", info)) // or use `to_string()` if `Display` is implemented
//                 //     //     .collect::<Vec<_>>()
//                 //     //     .join("\n\n")
//                 // );

//                 // if let Err(e) = erlang_sender_clone
//                 //     .send(ErlangMessageEvent {
//                 //         atom: atoms::iroh_gossip_node_discovered(),
//                 //         payload: vec![
//                 //             endpoint.node_id().fmt_short(),
//                 //             node_addr.node_id.fmt_short(),
//                 //             format!("{:?}", remote_info.latency)
//                 //         ],
//                 //     })
//                 //     .await
//                 // {
//                 //     tracing::warn!(
//                 //         "❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}",
//                 //         e
//                 //     );
//                 // }
//             }
//             Err(lagged) => {
//                 tracing::warn!(
//                     "🚨 {:?} Discovery stream lagged! Some items may have been lost.{:?}",
//                     endpoint.node_id().fmt_short(),
//                     lagged
//                 );
//             }
//         }
//     }
// }

// Note: format_remote_info and source_display_name removed in iroh 0.93+
// RemoteInfo and Source types are no longer available

#[derive(Debug, Serialize, Deserialize)]
struct Ticket {
    topic: TopicId,
    endpoint_addr: EndpointAddr,
}

impl Ticket {
    /// Deserialize from a slice of bytes to a Ticket.
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(Into::into)
    }

    /// Serialize from a `Ticket` to a `Vec` of bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serde_json::to_vec is infallible")
    }
}

// The `Display` trait allows us to use the `to_string`
// method on `Ticket`.
impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{}", text)
    }
}

// The `FromStr` trait allows us to turn a `str` into
// a `Ticket`
impl FromStr for Ticket {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD.decode(s.to_ascii_uppercase().as_bytes())?;
        Self::from_bytes(&bytes)
    }
}

#[rustler::nif]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

// ============================================================================
// AUTOMERGE CRDT NIF FUNCTIONS
// ============================================================================

use automerge::{AutoCommit, ObjType, ROOT, sync::SyncDoc, ReadDoc, transaction::Transactable, ActorId};

// Helper to navigate to a path in the document
fn navigate_to_path(doc: &mut AutoCommit, path: &[String]) -> NifResult<automerge::ObjId> {
    let mut obj = ROOT;
    for key in path {
        match doc.get(&obj, key.as_str()) {
            Ok(Some((automerge::Value::Object(_), id))) => obj = id,
            Ok(_) => return Err(rustler::Error::Term(Box::new(format!("Path not found: {}", key)))),
            Err(e) => return Err(rustler::Error::Term(Box::new(format!("Navigation error: {}", e)))),
        }
    }
    Ok(obj)
}

fn navigate_to_path_readonly(doc: &AutoCommit, path: &[String]) -> NifResult<automerge::ObjId> {
    let mut obj = ROOT;
    for key in path {
        match doc.get(&obj, key.as_str()) {
            Ok(Some((automerge::Value::Object(_), id))) => obj = id,
            Ok(_) => return Err(rustler::Error::Term(Box::new(format!("Path not found: {}", key)))),
            Err(e) => return Err(rustler::Error::Term(Box::new(format!("Navigation error: {}", e)))),
        }
    }
    Ok(obj)
}

// Helper to convert automerge value to Erlang term
fn value_to_term<'a>(env: Env<'a>, value: &automerge::Value) -> NifResult<Term<'a>> {
    match value {
        automerge::Value::Scalar(s) => match s.as_ref() {
            automerge::ScalarValue::Str(s) => Ok(s.to_string().encode(env)),
            automerge::ScalarValue::Int(i) => Ok(i.encode(env)),
            automerge::ScalarValue::Uint(u) => Ok((*u as i64).encode(env)),
            automerge::ScalarValue::F64(f) => Ok(f.encode(env)),
            automerge::ScalarValue::Boolean(b) => Ok(b.encode(env)),
            automerge::ScalarValue::Bytes(b) => Ok(b.as_slice().encode(env)),
            automerge::ScalarValue::Counter(c) => Ok((i64::from(c.clone())).encode(env)),
            automerge::ScalarValue::Timestamp(t) => Ok(t.encode(env)),
            automerge::ScalarValue::Null => Ok(atoms::not_found().encode(env)),
            automerge::ScalarValue::Unknown { .. } => Ok(atoms::not_found().encode(env)),
        },
        automerge::Value::Object(obj_type) => {
            Ok(format!("object:{:?}", obj_type).encode(env))
        }
    }
}

// ============================================================================
// Document Management
// ============================================================================

/// Create a new automerge document, returns doc_id
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_create_doc(node_ref: ResourceArc<NodeRef>) -> NifResult<String> {
    let mut state = node_ref.0.lock().unwrap();

    let mut doc = AutoCommit::new();
    if let Some(ref actor) = state.automerge_actor {
        doc.set_actor(actor.clone());
    }

    let doc_id = uuid::Uuid::new_v4().to_string();
    state.automerge_docs.insert(doc_id.clone(), doc);
    state.automerge_sync_states.insert(doc_id.clone(), std::collections::HashMap::new());

    Ok(doc_id)
}

/// Fork an existing document (create a copy with same history)
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_fork_doc(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
) -> NifResult<String> {
    let mut state = node_ref.0.lock().unwrap();

    let original = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let mut forked = original.fork();

    // Give the forked document a unique actor ID to avoid duplicate seq errors when merging
    let new_actor = ActorId::random();
    forked.set_actor(new_actor);

    let new_doc_id = uuid::Uuid::new_v4().to_string();
    state.automerge_docs.insert(new_doc_id.clone(), forked);
    state.automerge_sync_states.insert(new_doc_id.clone(), std::collections::HashMap::new());

    Ok(new_doc_id)
}

/// Load a document from saved bytes
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_load_doc(
    node_ref: ResourceArc<NodeRef>,
    data: Binary,
) -> NifResult<String> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = AutoCommit::load(data.as_slice())
        .map_err(|e| rustler::Error::Term(Box::new(format!("Load error: {}", e))))?;

    let doc_id = uuid::Uuid::new_v4().to_string();
    state.automerge_docs.insert(doc_id.clone(), doc);
    state.automerge_sync_states.insert(doc_id.clone(), std::collections::HashMap::new());

    Ok(doc_id)
}

/// Save document to binary format
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_save_doc<'a>(
    env: Env<'a>,
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
) -> NifResult<Binary<'a>> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let bytes = doc.save();
    let mut binary = OwnedBinary::new(bytes.len()).unwrap();
    binary.as_mut_slice().copy_from_slice(&bytes);
    Ok(binary.release(env))
}

/// Delete a document from memory
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_delete_doc(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
) -> NifResult<bool> {
    let mut state = node_ref.0.lock().unwrap();

    let removed = state.automerge_docs.remove(&doc_id).is_some();
    state.automerge_sync_states.remove(&doc_id);

    Ok(removed)
}

/// List all document IDs
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_docs(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let state = node_ref.0.lock().unwrap();
    Ok(state.automerge_docs.keys().cloned().collect())
}

// ============================================================================
// Map Operations
// ============================================================================

/// Put a value in a map at the given path
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_map_put(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
    value: Term,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;

    // Try to decode the value in different types
    if let Ok(s) = value.decode::<String>() {
        doc.put(&obj, &key, s).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(i) = value.decode::<i64>() {
        doc.put(&obj, &key, i).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(f) = value.decode::<f64>() {
        doc.put(&obj, &key, f).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(b) = value.decode::<bool>() {
        doc.put(&obj, &key, b).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(bin) = value.decode::<Binary>() {
        doc.put(&obj, &key, bin.as_slice().to_vec()).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else {
        return Err(rustler::Error::Term(Box::new("Unsupported value type")));
    }

    Ok(atoms::ok())
}

/// Put an object (map or list) at path, returns the object ID
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_map_put_object(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
    obj_type: String,
) -> NifResult<String> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let parent_obj = navigate_to_path(doc, &path)?;
    let obj_type = match obj_type.as_str() {
        "map" => ObjType::Map,
        "list" => ObjType::List,
        "text" => ObjType::Text,
        _ => return Err(rustler::Error::Term(Box::new("Invalid object type: use map, list, or text"))),
    };

    let obj_id = doc.put_object(&parent_obj, &key, obj_type)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Put object error: {}", e))))?;

    Ok(obj_id.to_string())
}

/// Get a value from a map at the given path
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_map_get(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
) -> NifResult<Term> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;

    match doc.get(&obj, &key) {
        Ok(Some((value, _))) => value_to_term(env, &value),
        Ok(None) => Ok(atoms::not_found().encode(env)),
        Err(e) => Err(rustler::Error::Term(Box::new(format!("Get error: {}", e)))),
    }
}

/// Delete a key from a map
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_map_delete(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;
    doc.delete(&obj, &key)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Delete error: {}", e))))?;

    Ok(atoms::ok())
}

/// Get all keys from a map
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_map_keys(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
) -> NifResult<Vec<String>> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;
    let keys: Vec<String> = doc.keys(&obj).collect();

    Ok(keys)
}

// ============================================================================
// List Operations
// ============================================================================

/// Insert a value into a list at index
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_insert(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    index: usize,
    value: Term,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;

    if let Ok(s) = value.decode::<String>() {
        doc.insert(&obj, index, s).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(i) = value.decode::<i64>() {
        doc.insert(&obj, index, i).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(f) = value.decode::<f64>() {
        doc.insert(&obj, index, f).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(b) = value.decode::<bool>() {
        doc.insert(&obj, index, b).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else {
        return Err(rustler::Error::Term(Box::new("Unsupported value type")));
    }

    Ok(atoms::ok())
}

/// Push a value to the end of a list
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_push(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    value: Term,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;
    let len = doc.length(&obj);

    if let Ok(s) = value.decode::<String>() {
        doc.insert(&obj, len, s).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(i) = value.decode::<i64>() {
        doc.insert(&obj, len, i).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(f) = value.decode::<f64>() {
        doc.insert(&obj, len, f).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else if let Ok(b) = value.decode::<bool>() {
        doc.insert(&obj, len, b).map_err(|e| rustler::Error::Term(Box::new(e.to_string())))?;
    } else {
        return Err(rustler::Error::Term(Box::new("Unsupported value type")));
    }

    Ok(atoms::ok())
}

/// Get a value from a list at index
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_get(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    index: usize,
) -> NifResult<Term> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;

    match doc.get(&obj, index) {
        Ok(Some((value, _))) => value_to_term(env, &value),
        Ok(None) => Ok(atoms::not_found().encode(env)),
        Err(e) => Err(rustler::Error::Term(Box::new(format!("Get error: {}", e)))),
    }
}

/// Delete a value from a list at index
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_delete(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    index: usize,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;
    doc.delete(&obj, index)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Delete error: {}", e))))?;

    Ok(atoms::ok())
}

/// Get list length
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_list_length(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
) -> NifResult<usize> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;
    Ok(doc.length(&obj))
}

// ============================================================================
// Text Operations
// ============================================================================

/// Create a text object at path with initial text
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_text_create(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
    initial_text: String,
) -> NifResult<String> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let parent_obj = navigate_to_path(doc, &path)?;
    let text_id = doc.put_object(&parent_obj, &key, ObjType::Text)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Create text error: {}", e))))?;

    if !initial_text.is_empty() {
        doc.splice_text(&text_id, 0, 0, &initial_text)
            .map_err(|e| rustler::Error::Term(Box::new(format!("Splice error: {}", e))))?;
    }

    Ok(text_id.to_string())
}

/// Insert text at position
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_text_insert(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    position: usize,
    text: String,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;
    doc.splice_text(&obj, position, 0, &text)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Insert error: {}", e))))?;

    Ok(atoms::ok())
}

/// Delete text at position
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_text_delete(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    position: usize,
    length: usize,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;
    doc.splice_text(&obj, position, length as isize, "")
        .map_err(|e| rustler::Error::Term(Box::new(format!("Delete error: {}", e))))?;

    Ok(atoms::ok())
}

/// Get full text content
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_text_get(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
) -> NifResult<String> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;
    let text = doc.text(&obj)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Get text error: {}", e))))?;

    Ok(text)
}

// ============================================================================
// Counter Operations
// ============================================================================

/// Create or increment a counter, returns the new value
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_counter_increment(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
    delta: i64,
) -> NifResult<i64> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path(doc, &path)?;

    // Check if counter exists
    if let Ok(Some((value, _))) = doc.get(&obj, &key) {
        if let automerge::Value::Scalar(s) = value {
            if let automerge::ScalarValue::Counter(c) = s.as_ref() {
                let current_val: i64 = c.clone().into();
                doc.increment(&obj, &key, delta)
                    .map_err(|e| rustler::Error::Term(Box::new(format!("Increment error: {}", e))))?;
                return Ok(current_val + delta);
            }
        }
    }

    // Create new counter
    doc.put(&obj, &key, automerge::ScalarValue::Counter(delta.into()))
        .map_err(|e| rustler::Error::Term(Box::new(format!("Put counter error: {}", e))))?;

    Ok(delta)
}

/// Get counter value
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_counter_get(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    path: Vec<String>,
    key: String,
) -> NifResult<i64> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let obj = navigate_to_path_readonly(doc, &path)?;

    match doc.get(&obj, &key) {
        Ok(Some((automerge::Value::Scalar(s), _))) => {
            if let automerge::ScalarValue::Counter(c) = s.as_ref() {
                Ok(i64::from(c.clone()))
            } else {
                Err(rustler::Error::Term(Box::new("Not a counter")))
            }
        }
        Ok(_) => Err(rustler::Error::Term(Box::new("Counter not found"))),
        Err(e) => Err(rustler::Error::Term(Box::new(format!("Get error: {}", e)))),
    }
}

// ============================================================================
// Merge and Sync Operations
// ============================================================================

/// Merge another document (as bytes) into this one
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_merge(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    other_doc_bytes: Binary,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let mut other = AutoCommit::load(other_doc_bytes.as_slice())
        .map_err(|e| rustler::Error::Term(Box::new(format!("Load error: {}", e))))?;

    doc.merge(&mut other)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Merge error: {}", e))))?;

    Ok(atoms::ok())
}

/// Generate a sync message to send to a peer
/// Note: For simplicity, this returns the full document save rather than incremental sync.
/// Use automerge_sync_via_gossip for the primary sync mechanism.
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_generate_sync_message<'a>(
    env: Env<'a>,
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    _peer_id: String,
) -> NifResult<Term<'a>> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    // For simplicity, return the full document save as the "sync message"
    let bytes = doc.save();
    let mut binary = OwnedBinary::new(bytes.len()).unwrap();
    binary.as_mut_slice().copy_from_slice(&bytes);
    Ok(binary.release(env).encode(env))
}

/// Receive and apply a sync message from a peer
/// Note: This expects a full document save, which it merges into the local doc.
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_receive_sync_message(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
    _peer_id: String,
    message: Binary,
) -> NifResult<Atom> {
    let mut state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get_mut(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    // Load and merge the received document
    let mut other = AutoCommit::load(message.as_slice())
        .map_err(|e| rustler::Error::Term(Box::new(format!("Load error: {}", e))))?;

    doc.merge(&mut other)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Merge error: {}", e))))?;

    Ok(atoms::ok())
}

/// Sync document via gossip (broadcast full document to all peers)
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_sync_via_gossip(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
) -> NifResult<Atom> {
    let (sender, doc_bytes, node_id) = {
        let mut state = node_ref.0.lock().unwrap();

        // Get sender and node_id first
        let sender = state.sender.clone();
        let node_id = state.endpoint.id();

        // Then get the doc
        let doc = state.automerge_docs.get_mut(&doc_id)
            .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;
        let doc_bytes = doc.save();

        (sender, doc_bytes, node_id)
    };

    // Create a special message type for automerge sync
    #[derive(Serialize)]
    struct AutomergeSyncMessage {
        msg_type: String,
        doc_id: String,
        from: String,
        document: Vec<u8>,
    }

    let message = AutomergeSyncMessage {
        msg_type: "automerge_sync".to_string(),
        doc_id,
        from: node_id.to_string(),
        document: doc_bytes,
    };

    let bytes = serde_json::to_vec(&message)
        .map_err(|e| rustler::Error::Term(Box::new(format!("Serialize error: {}", e))))?;

    RUNTIME.block_on(async {
        sender.broadcast(bytes.into()).await
            .map_err(|e| rustler::Error::Term(Box::new(format!("Broadcast error: {}", e))))
    })?;

    Ok(atoms::ok())
}

/// Get document as JSON-like map for debugging/inspection
#[rustler::nif(schedule = "DirtyCpu")]
pub fn automerge_to_json(
    node_ref: ResourceArc<NodeRef>,
    doc_id: String,
) -> NifResult<String> {
    let state = node_ref.0.lock().unwrap();

    let doc = state.automerge_docs.get(&doc_id)
        .ok_or_else(|| rustler::Error::Term(Box::new("Document not found")))?;

    let json = serde_json::to_string(&automerge::AutoSerde::from(doc))
        .map_err(|e| rustler::Error::Term(Box::new(format!("JSON serialization error: {}", e))))?;
    Ok(json)
}

// fn setup_console_subscriber_once() {
//     let _ = Registry::default()
//         .with(ConsoleLayer::builder().with_default_env().spawn())
//         .try_init();
// }

// Rustler init

// fn start_deadlock_checker() {
//     thread::spawn(move || loop {
//         thread::sleep(Duration::from_secs(10));
//         let deadlocks = deadlock::check_deadlock();
//         if deadlocks.is_empty() {
//             return;
//         }
//         eprintln!("🧨 {} deadlocks detected!", deadlocks.len());
//         for (i, threads) in deadlocks.iter().enumerate() {
//             eprintln!("Deadlock #{}", i);
//             for t in threads {
//                 eprintln!("{:?}", t.backtrace());
//             }
//         }
//     });
// }

// pub fn start_continuous_flamegraph(interval_secs: u64) {
//     thread::spawn(move || {
//         loop {
//             let guard = match pprof::ProfilerGuard::new(100) {
//                 Ok(g) => g,
//                 Err(e) => {
//                     eprintln!("🔥 Failed to create profiler guard: {:?}", e);
//                     thread::sleep(Duration::from_secs(interval_secs));
//                     continue;
//                 }
//             };

//             thread::sleep(Duration::from_secs(interval_secs));

//             match guard.report().build() {
//                 Ok(report) => {
//                     let timestamp = SystemTime::now()
//                         .duration_since(UNIX_EPOCH)
//                         .unwrap()
//                         .as_secs();
//                     let filename = format!("flamegraph_{}.svg", timestamp);
//                     let path = PathBuf::from(filename);

//                     match File::create(&path) {
//                         Ok(mut file) => {
//                             if let Err(e) = report.flamegraph(&mut file) {
//                                 eprintln!("🔥 Error writing flamegraph: {:?}", e);
//                             } else {
//                                 println!("🧯 Flamegraph saved: {:?}", path);
//                             }
//                         }
//                         Err(e) => eprintln!("🔥 Failed to create flamegraph file: {:?}", e),
//                     }
//                 }
//                 Err(e) => eprintln!("🔥 Failed to build flamegraph report: {:?}", e),
//             }
//         }
//     });
// }

fn on_load(env: Env, _info: Term) -> bool {
    // let _ = console_subscriber::init();
    // setup_console_subscriber_once();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("iroh=error,iroh_ex=info"));

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_ansi(atty::is(atty::Stream::Stdout))
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set up logging");

    println!("Initializing Rust Iroh NIF module ...");
    let _ = rustler::resource!(NodeRef, env);
    println!("Rust NIF Iroh module loaded successfully.");

    // start_continuous_flamegraph(180);
    // start_deadlock_checker();

    true
}

rustler::init!("Elixir.IrohEx.Native", load = on_load);
