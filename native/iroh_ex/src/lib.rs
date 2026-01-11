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

use iroh::discovery::{dns::DnsDiscovery, mdns::MdnsDiscovery, pkarr::PkarrPublisher};
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
use distributed_topic_tracker::{
    AutoDiscoveryGossip,
    RecordPublisher,
    RecordTopic,
    signing_keypair,
    unix_minute,
};

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
        .discovery(PkarrPublisher::n0_dns())
        .discovery(DnsDiscovery::n0_dns())
        .discovery(MdnsDiscovery::builder());

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

/// Subscribe to a topic with DHT-based auto-discovery
/// This allows nodes to find each other without exchanging tickets
#[rustler::nif(schedule = "DirtyCpu")]
pub fn subscribe_with_auto_discovery(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    topic_name: String,
    secret_seed: Option<String>,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let node_ref_clone = node_ref.clone();
    let pid = env.pid();

    RUNTIME.spawn(async move {
        if let Err(e) = subscribe_with_auto_discovery_internal(node_ref_clone, pid, topic_name, secret_seed).await {
            tracing::error!("❌ Error in auto-discovery subscription: {:?}", e);
        }
    });

    Ok(node_ref)
}

async fn subscribe_with_auto_discovery_internal(
    node_ref: ResourceArc<NodeRef>,
    pid: LocalPid,
    topic_name: String,
    secret_seed: Option<String>,
) -> Result<()> {
    let resource_arc = node_ref.0.clone();

    let (endpoint, gossip, node_id_short, erlang_sender_clone) = {
        let state = resource_arc.lock().unwrap();
        (
            state.endpoint.clone() as Endpoint,
            state.gossip.clone() as Gossip,
            state.endpoint.id().fmt_short().to_string(),
            state.mpsc_event_sender.clone(),
        )
    };

    // Create RecordTopic from the topic name (hashes via SHA512)
    let record_topic = RecordTopic::from_str(&topic_name)
        .context("❌ Failed to create RecordTopic")?;

    // Get current unix minute for key derivation
    let current_minute = unix_minute(0);

    // Derive signing key from topic and time
    let signing_key = signing_keypair(record_topic.clone(), current_minute);
    let verifying_key = signing_key.verifying_key();

    // Create the initial secret for DHT record encryption
    let initial_secret: Vec<u8> = if let Some(seed) = secret_seed {
        // Use provided seed
        let mut bytes = seed.as_bytes().to_vec();
        // Pad or truncate to 32 bytes for consistency
        bytes.resize(32, 0);
        bytes
    } else {
        // Generate random 32-byte secret
        let mut bytes = vec![0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut bytes[..]);
        bytes
    };

    // Create RecordPublisher for DHT-based discovery
    let record_publisher = RecordPublisher::new(
        record_topic,
        verifying_key,
        signing_key,
        None,  // No custom secret rotation - use default
        initial_secret,
    );

    tracing::info!(
        "📡 Subscribing to topic '{}' with DHT auto-discovery, node: {}",
        topic_name,
        node_id_short
    );

    // Send notification that we're starting auto-discovery
    if let Err(e) = erlang_sender_clone
        .send(ErlangMessageEvent {
            atom: atoms::iroh_gossip_joined(),
            payload: Payload::List(vec![
                Payload::String(node_id_short.clone()),
                Payload::String(topic_name.clone()),
                Payload::String("auto_discovery".to_string()),
            ]),
        })
        .await
    {
        tracing::warn!("❌ Failed to send auto-discovery started notification: {:?}", e);
    }

    let node_ref_clone = node_ref.clone();
    let erlang_sender_inner = erlang_sender_clone.clone();
    let node_id_short_clone = node_id_short.clone();

    // Use the auto-discovery extension
    let topic = gossip
        .subscribe_and_join_with_auto_discovery(record_publisher)
        .await
        .context("❌ Failed to subscribe with auto-discovery")?;

    tracing::info!("✅ Successfully subscribed to topic with auto-discovery");

    // Note: distributed-topic-tracker's Topic has an async split() method that returns Result
    // The sender/receiver are distributed-topic-tracker's wrapper types
    let (dht_sender, dht_receiver) = topic.split().await
        .context("❌ Failed to split topic into sender/receiver")?;

    // Note: We don't update state.sender here since distributed-topic-tracker uses its own wrapper types
    // Messages should be broadcast via the dht_sender instead

    // Spawn event handler task
    let event_handler_task = Some(RUNTIME.spawn(async move {
        while let Some(event) = dht_receiver.next().await {
            match event {
                Ok(event) => {
                    match event {
                        Event::NeighborUp(pub_key) => {
                            // neighbors() returns a Future in distributed-topic-tracker
                            let neighbors = dht_receiver.neighbors().await;
                            let neighbor_count = neighbors.len();

                            if erlang_sender_inner.is_closed() {
                                tracing::error!("❌ GossipEvent::NeighborUp: erlang_sender is closed");
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

                            if let Err(e) = erlang_sender_inner.send(event).await {
                                tracing::error!("❌ Failed to send NeighborUp event: {:?}", e);
                            }
                        }

                        Event::NeighborDown(pub_key) => {
                            if erlang_sender_inner.is_closed() {
                                continue;
                            }

                            let event = ErlangMessageEvent {
                                atom: atoms::iroh_gossip_neighbor_down(),
                                payload: Payload::List(vec![
                                    Payload::String(node_id_short_clone.clone()),
                                    Payload::String(pub_key.fmt_short().to_string()),
                                ]),
                            };

                            if let Err(e) = erlang_sender_inner.send(event).await {
                                tracing::error!("❌ Failed to send NeighborDown event: {:?}", e);
                            }
                        }

                        Event::Received(msg) => {
                            if erlang_sender_inner.is_closed() {
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

                                        if let Err(e) = erlang_sender_inner.send(event).await {
                                            tracing::error!("❌ Failed to send message event: {:?}", e);
                                        }
                                    }
                                    Message::Message { from, text } => {
                                        tracing::debug!("📝 {}: {}", from, text);
                                    }
                                },
                                Err(e) => {
                                    tracing::warn!("❌ Failed to parse message: {:?}", e);
                                }
                            }
                        }
                        unhandled_event => {
                            tracing::debug!("🔍 Ignored unhandled event: {:?}", unhandled_event);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("❌ Failed to receive event: {:?}", e);
                }
            }
        }
    }));

    {
        let mut state = node_ref.0.lock().unwrap();
        state.event_handler_task = event_handler_task;
    }

    Ok(())
}

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
