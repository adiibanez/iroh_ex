#![no_main]
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(deprecated)]
#![allow(unused_must_use)]
#![allow(non_local_definitions)]
// #![allow(unexpected_cfgs)]
// #[cfg(not(clippy))]
#![feature(mpmc_channel)]
#![allow(clippy::too_many_arguments)]

use iroh::discovery::DiscoveryItem;
use iroh::endpoint::RemoteInfo;
use iroh::endpoint::Source;
use iroh::PublicKey;
use iroh::RelayMap;
use iroh::RelayMode;
use iroh::RelayUrl;
use iroh::endpoint::ConnectionType;
use rustler::{
    Encoder, Env, Error as RustlerError, LocalPid, NifResult, OwnedEnv, ResourceArc, Term,
};
use rustler::types::atom::Atom;

use tokio::sync::mpsc;
use tokio::sync::RwLock;

use tracing_subscriber::EnvFilter;

use rand::Rng;

use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::env;
// use std::sync::mpmc::Sender;
// use std::sync::mpmc::Receiver;
use std::sync::{Arc, Mutex};

// use tracing_subscriber::{Registry, prelude::*};
// use console_subscriber::ConsoleLayer;

use std::fmt;
use std::ptr;
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::ProtocolHandler,
    Endpoint, NodeAddr, NodeId, SecretKey,
};

use rand::rngs::OsRng;

use quic_rpc::transport::flume::FlumeConnector;

pub(crate) type BlobsClient = iroh_blobs::rpc::client::blobs::Client<
    FlumeConnector<iroh_blobs::rpc::proto::Response, iroh_blobs::rpc::proto::Request>,>;
pub(crate) type DocsClient = iroh_docs::rpc::client::docs::Client<
    FlumeConnector<iroh_docs::rpc::proto::Response, iroh_docs::rpc::proto::Request>,>;

use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver},
    proto::TopicId,
    ALPN as GossipALPN,
};

use serde::{Deserialize, Serialize};

use n0_future::boxed::BoxFuture;
use n0_future::StreamExt;

use rand::distributions::Alphanumeric;

mod state;
mod tokio_runtime;
mod wrappers;
mod gossip_actor;
mod erlang_actor;
mod actor;
mod utils;

use crate::state::NodeRef;
use crate::state::{ErlangMessageEvent, Payload};
use crate::state::atoms;
use crate::tokio_runtime::RUNTIME;
use crate::state::NodeState;
// use parking_lot::deadlock;

const ALPN: &[u8] = b"iroh-example/echo/0";

static TOPIC_NAME: Lazy<String> = Lazy::new(generate_topic_name);

fn generate_topic_name() -> String {
    rand::thread_rng()
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

fn string_to_32_byte_array(s: &str) -> [u8; 32] {
    let mut result = [0u8; 32];
    let bytes = s.as_bytes();
    let len = std::cmp::min(bytes.len(), 32);
    result[..len].copy_from_slice(&bytes[..len]);
    result
}

#[rustler::nif]
pub fn generate_secretkey(env: Env) -> Result<String, RustlerError> {
    let mut rng = OsRng;
    let secret_key = SecretKey::generate(&mut rng);
    Ok(secret_key.to_string())
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn create_node(env: Env, pid: LocalPid, is_whale_node: bool) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let env_pid_clone = env.pid();
    let monitor_pid = pid;
    let topic_name = TOPIC_NAME.to_string();

    let (resource, monitor_ref) = RUNTIME.block_on(async move {

        // let relay_url_str = env::var("RELAY_URL").unwrap_or_else(|_| "http://localhost:3340".to_string());

        // let relay_url = RelayUrl::from_str(&relay_url_str)
        //     .expect("Failed to parse relay url from environment or default");
        // let relay_map = RelayMap::from_url(relay_url);

        // // let relay_url_str = "https://euw1-1.relay.iroh.network./";
        // let relay_url_str = "http://localhost:3340";

        // let relay_url = RelayUrl::from_str(relay_url_str).expect("Failed to parse relay url");
        // let relay_map = RelayMap::from_url(relay_url);

        let relay_mode = if let Ok(url_str) = env::var("RELAY_URL") {
            let relay_url = RelayUrl::from_str(&url_str)
                .expect("Failed to parse RELAY_URL");
            let relay_map = RelayMap::from_url(relay_url);
            RelayMode::Custom(relay_map)
        } else if env::var("RELAY_DISABLED").is_ok() {
            RelayMode::Disabled
        } else if env::var("RELAY_EU_ONLY").is_ok() {
            let relay_url = RelayUrl::from_str("https://euw1-1.relay.iroh.network./")
                .expect("Failed to parse hardcoded EU relay URL");
            let relay_map = RelayMap::from_url(relay_url);
            RelayMode::Custom(relay_map)
        } else {
            RelayMode::Default
        };
        
        tracing::info!("RELAY config {:?}", relay_mode);

        let endpoint_builder = Endpoint::builder()
            .relay_mode(relay_mode)
            // .discovery_n0()
            .discovery_local_network();

        let endpoint: Endpoint = endpoint_builder.bind()
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Endpoint error: {}", e))))?;

            
        let endpoint_clone = endpoint.clone();
        let router_builder = iroh::protocol::Router::builder(endpoint.clone());

        let hyparview_config = if is_whale_node {
            iroh_gossip::proto::HyparviewConfig {
                active_view_capacity: 50,
                passive_view_capacity: 200,
                ..Default::default()
            }
        } else {
            iroh_gossip::proto::HyparviewConfig::default()
        };
        
        let gossip_builder = Gossip::builder().membership_config(hyparview_config);

        let gossip = gossip_builder
            .spawn(endpoint.clone())
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {}", e))))?;

        let router = router_builder
            .accept(GossipALPN, gossip.clone())
            .accept(ALPN, Echo)
            .spawn()
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Router error: {}", e))))?;

        let router_clone = router.clone();

        let node_ids = vec![];
        let topic = gossip
            .subscribe(
                TopicId::from_bytes(string_to_32_byte_array(&topic_name)),
                node_ids,
            )
            // .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

        let (mpsc_event_sender, mpsc_event_receiver) = mpsc::channel::<ErlangMessageEvent>(1000);
        let (sender, _receiver) = topic.split();
        let mpsc_event_receiver_arc = Arc::new(RwLock::new(mpsc_event_receiver));

        let state = NodeState::new(
            monitor_pid,
            endpoint_clone.clone(),
            router_clone.clone(),
            gossip.clone(),
            sender,
            mpsc_event_sender,
            mpsc_event_receiver_arc.clone(),
        );

        let resource = ResourceArc::new(NodeRef(Arc::new(Mutex::new(state))));
        let monitor_ref = env.monitor(&resource, &monitor_pid);

        // Start task inside async
        let node_addr_short = endpoint.node_id().fmt_short().clone();
        let handler_pid = monitor_pid;
        let handler_monitor = monitor_ref;

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
    println!("Create ticket");

    let resource_arc = node_ref.0.clone();

    let endpoint = {
        let state = resource_arc.lock().unwrap();
        state.endpoint.clone()
    };

    let topic = TopicId::from_bytes(string_to_32_byte_array(&TOPIC_NAME.to_string()));

    let node_addr = RUNTIME
        .block_on(endpoint.node_addr())
        .map_err(|e| RustlerError::Term(Box::new(format!("Node addr error: {}", e))))?;

    let ticket = {
        // Get our address information, includes our
        // `NodeId`, our `RelayUrl`, and any direct
        // addresses.
        let me = node_addr;
        let nodes = vec![me];
        Ticket { topic, nodes }
    };

    Ok(ticket.to_string())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn gen_node_addr(node_ref: ResourceArc<NodeRef>) -> NifResult<String> {
    let resource_arc = node_ref.0.clone();
    let endpoint: Endpoint = {
        let state = resource_arc.lock().unwrap();
        state.endpoint.clone()
    };

    let node_id = endpoint.node_id();

    // let addr = node.local_peer_id().to_string();
    Ok(node_id.fmt_short())
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn send_message(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    message: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    // println!("Message: {:?}", message);

    let resource_arc = node_ref.0.clone();

    let (endpoint, gossip, sender ) = {
        let state = resource_arc.lock().unwrap();
        (state.endpoint.clone(), state.gossip.clone(), state.sender.clone())
    };

    let message = Message::AboutMe {
        from: endpoint.node_id(),
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

    let msg_env = OwnedEnv::new();

    let Ticket { topic, nodes } = Ticket::from_str(&ticket).context("❌ Failed to parse ticket")?;

    let (endpoint, gossip, node_id, node_id_short, erlang_sender_clone) = {
        let state = resource_arc.lock().unwrap();
        (
            state.endpoint.clone() as Endpoint,
            state.gossip.clone() as Gossip,
            state.endpoint.node_id() as PublicKey,
            state.endpoint.node_id().fmt_short(),
            state.mpsc_event_sender.clone(),
        )
    };

    let endpoint_clone = endpoint.clone();
    let node_ref_clone = node_ref.clone();

    // {
    //     let mut state = node_ref.0.lock().unwrap(); // Locks the mutex
    //     let pid = state.pid; // Clone only what is needed
    //     // Now that state is unlocked, we can create the task safely
    //     state.discovery_event_handler_task = Some(RUNTIME.spawn(log_discovery_stream(node_ref_clone.clone(), pid)));
    //     drop(state); // Explicitly drop the lock to avoid Send issues
    // };

    // Re-lock the state and store the handle safely
    {
        let state = node_ref.0.lock().unwrap();
        // state.discovery_event_handler_task = Some(discovery_task);
    }

    tracing::debug!(
        "connect_node endpoint_Ptr:{:?} topic: {:?} nodes: {:?}",
        &endpoint as *const _,
        topic,
        nodes
    );

    // avoid adding me, myself and I
    let nodes_filtered: Vec<_> = nodes
        .iter()
        .filter(|n| n.node_id != node_id)
        .filter(|n| n.node_id.fmt_short() != node_id_short)
        .collect();

    let node_ids: Vec<_> = nodes_filtered
        .iter()
        .map(|p| p.node_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // let node_ids: Vec<_> = nodes.iter().map(|p| p.node_id).collect();
    if nodes.is_empty() {
        tracing::debug!("Empty nodes list {:?}", nodes);
    } else {
        for node in nodes_filtered {
            tracing::debug!("Adding node to addr book {:?}", node);
            if let Err(e) = endpoint_clone.add_node_addr(node.clone()) {
                tracing::error!("❌ Failed to add node to address book: {:?}", e);
            }
        }
    }

    let relay_url: RelayUrl = endpoint_clone.home_relay().initialized().await.unwrap();

    if let Err(e) = erlang_sender_clone
        .send(ErlangMessageEvent {
            atom: atoms::iroh_node_connected(),
            payload: Payload::List(vec![
                Payload::String(node_id.fmt_short()),
                Payload::String(relay_url.as_str().to_string()),
            ]),
        })
        .await
    {
        tracing::warn!(
            "❌ GossipEvent::Joined Failed to send erlang message: {:?}",
            e
        );
    }

    let pid_clone = pid;

    let erlang_sender_clone_inner = erlang_sender_clone.clone();

    let event_handler_task = Some(RUNTIME.spawn(async move {

        let topic = gossip
            .subscribe_and_join(topic, node_ids)
            .await
            .context("❌ Failed to subscribe and join gossip")
            .unwrap();

        tracing::debug!("Subscribed to: {:?}", topic);

        let (sender, mut receiver) = topic.split();

        let pid_clone = pid;
        let erlang_sender_clone = erlang_sender_clone.clone();
        let node_id_short_clone = node_id_short.clone();

        
        while let Some(event) = receiver.next().await {
            match event {
                Ok(event) => {
        
                    match event {
                        Event::Gossip(GossipEvent::Joined(pub_keys)) => {
                            tracing::debug!("Joined {:?} {:?}", node_id_short_clone, pub_keys);

                            if let Err(e) = erlang_sender_clone
                                .send(ErlangMessageEvent {
                                    atom: atoms::iroh_gossip_joined(),
                                    payload: Payload::List(vec![
                                        Payload::String(pub_keys
                                            .iter()
                                            .map(|pk| pk.fmt_short())
                                            .collect::<Vec<_>>()
                                            .join(",")),
                                    ]),
                                })
                                .await
                            {
                                tracing::warn!("❌ GossipEvent::Joined Failed to send erlang message: {:?}", e);
                            }
                        }

                        // GossipReceiver::neighbors

                        Event::Gossip(GossipEvent::NeighborUp(pub_key)) => {
                            tracing::debug!("NeighborUp {:?}", pub_key);

                            let remote_pubkey_opt = receiver
                                .neighbors()
                                .find(|n| n.fmt_short() != node_id_short_clone);

                            if let Some(remote_pubkey) = remote_pubkey_opt {
                                let remote_info_string = if let Some(remote_info) = endpoint_clone
                                    .remote_info_iter()
                                    .find(|r| r.node_id != remote_pubkey)
                                {
                                    serde_json::to_string(&remote_info_to_map(&remote_info))
                                        .unwrap_or_else(|_| "{}".to_string())
                                } else {
                                    "{}".to_string()
                                };

                                if let Err(e) = erlang_sender_clone
                                    .send(ErlangMessageEvent {
                                        atom: atoms::iroh_gossip_neighbor_up(),
                                        payload: Payload::List(vec![
                                            Payload::String(node_id_short_clone.clone()),
                                            Payload::String(pub_key.clone().fmt_short()),
                                            Payload::String(remote_info_string),
                                        ]),
                                    })
                                    .await
                                {
                                    tracing::warn!("❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}", e);
                                }
                                // Do something with `others`...
                            }

                            // .filter(|n| n.node_id != node_addr.node_id)
                    // .collect::<Vec<_>>();

                // for info in &remote_info_vec {
                //     tracing::info!("{}", format_remote_info(info));
                // }

                        }
                        Event::Gossip(GossipEvent::NeighborDown(pub_key)) => {
                            tracing::debug!("NeighborDown {:?}", pub_key);

                            if let Err(e) = erlang_sender_clone
                                .send(ErlangMessageEvent {
                                    atom: atoms::iroh_gossip_neighbor_down(),
                                    payload: Payload::List(vec![
                                        Payload::String(node_id_short_clone.clone()),
                                        Payload::String(pub_key.clone().fmt_short()),
                                    ]),
                                })
                                .await
                            {
                                tracing::warn!("❌ GossipEvent::NeighborDown Failed to send erlang message: {:?}", e);
                            }
                        }
                        Event::Gossip(GossipEvent::Received(msg)) => {
                            tracing::debug!("Received message: {:?}", msg);

                            match Message::from_bytes(&msg.content) {
                                Ok(message) => match message {
                                    Message::AboutMe { from, name } => {
                                        tracing::debug!("FROM: {} MSG: {}", from.fmt_short(), name);

                                        if let Err(e) = erlang_sender_clone
                                            .send(ErlangMessageEvent {
                                                atom: atoms::iroh_gossip_message_received(),
                                                payload: Payload::List(vec![
                                                    Payload::String(node_id_short_clone.clone()),
                                                    Payload::String(name.clone()),
                                                ]),
                                            })
                                            .await
                                        {
                                            tracing::warn!(
                                                "❌ GossipEvent::Received Failed to send erlang message: {:?}",
                                                e
                                            );
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

                            if let Err(e) = erlang_sender_clone
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

    Ok(())
}


use std::collections::HashMap;

fn remote_info_to_map(info: &RemoteInfo) -> HashMap<String, String> {
    let mut map = HashMap::new();

    map.insert("node_id".to_string(), format!("{}", info.node_id));

    // if let Some(relay) = &info.relay_url {
    //     map.insert("relay_url".to_string(), relay.relay_url.to_string());
    // }

    if let Some(latency) = info.latency {
        map.insert("latency".to_string(), format!("{:?}", latency));
    }

    if let Some(last_used) = info.last_used {
        map.insert("last_used".to_string(), format!("{:?}", last_used));
    }

    match &info.conn_type {
        ConnectionType::Direct(addr) => {
            map.insert("conn_type".to_string(), "Direct".to_string());
            // map.insert("conn_addr".to_string(), addr.to_string());
        }
        ConnectionType::Relay(url) => {
            map.insert("conn_type".to_string(), "Relay".to_string());
            map.insert("relay_conn_url".to_string(), url.to_string());
        }
        ConnectionType::Mixed(addr, url) => {
            map.insert("conn_type".to_string(), "Mixed".to_string());
            // map.insert("conn_addr".to_string(), addr.to_string());
            map.insert("relay_conn_url".to_string(), url.to_string());
        }
        ConnectionType::None => {
            map.insert("conn_type".to_string(), "None".to_string());
        }
    }

    // addrs intentionally skipped

    map
}


#[rustler::nif(schedule = "DirtyCpu")]
fn disconnect_node(node_ref: ResourceArc<NodeRef>) -> NifResult<()> {
    let node = node_ref.0.clone();

    let endpoint = {
        let state = node_ref.0.lock().unwrap();
        state.endpoint.clone() as Endpoint
    };

    endpoint.close();

    // let node = node_ref.lock().unwrap();
    // node.disconnect_all();  // Assuming an API to disconnect all peers
    Ok(())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn list_peers(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let node = node_ref.0.clone();

    let endpoint = {
        let state = node_ref.0.lock().unwrap();
        state.endpoint.clone() as Endpoint
    };

    let remote_info_vec: Vec<RemoteInfo> = endpoint
                    .remote_info_iter()
                    // .filter(|n| n.node_id != node_addr.node_id)
                    .collect::<Vec<_>>();

    for info in &remote_info_vec {
        tracing::info!("{}", format_remote_info(info));
    }
    
    //let peers: Vec<_> = vec![]; 
    let peers: Vec<_> = remote_info_vec.iter().map(|ri| {
        let latency_str = match ri.latency {
            Some(latency) => format!("{:?}", latency),
            None => "N/A".to_string(),
        };
        format!("node_id:{:?},conn_type:{:?},latency:{}", ri.node_id.fmt_short(), ri.conn_type, latency_str)
    }).collect();
    Ok(peers)
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn cleanup(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
) -> NifResult<()> {
    let resource_arc = node_ref.0.clone();

    let (pid, monitor_ref) = {
        let state = resource_arc.lock().unwrap();
        (state.pid, state.monitor_ref)
    };

    if let Some(ref monitor) = monitor_ref {
        env.demonitor(&node_ref, monitor);
    }
    drop(node_ref);
    Ok(())
}

// The protocol definition:
#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    fn accept(&self, connection: Connection) -> BoxFuture<Result<()>> {
        Box::pin(async move {
            let (mut send, mut recv) = connection.accept_bi().await?;

            // Echo any bytes received back directly.
            let bytes_sent = tokio::io::copy(&mut recv, &mut send).await?;

            send.finish()?;
            connection.closed().await;

            Ok(())
        })
    }
}

// async fn log_discovery_stream(endpoint: Endpoint, pid: LocalPid) {
async fn log_discovery_stream(node_ref: ResourceArc<NodeRef>, pid: LocalPid) {
    let (endpoint, erlang_sender_clone, mut stream) = {
        let state = node_ref.0.lock().unwrap(); // Acquire lock
        (
            state.endpoint.clone() as Endpoint,
            state.mpsc_event_sender.clone() as tokio::sync::mpsc::Sender<ErlangMessageEvent>,
            state.endpoint.discovery_stream(), // as Stream<Item = Result<DiscoveryItem, Lagged>>
        )
    };

    // let endpoint = state_clone.endpoint.clone();
    // let erlang_sender_clone = state_clone.mpsc_event_sender.clone();

    // let mut stream = endpoint.discovery_stream();
    let msg_env = OwnedEnv::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(discovery_item) => {

                let node_addr: NodeAddr = (discovery_item as DiscoveryItem).into_node_addr();

                // let remote_info: RemoteInfo = endpoint.remote_info_iter()
                //     .into(Vec)
                //     // .find(|n| n.node_id == node_addr.node_id)
                //     .expect("Expected at least one RemoteInfo");

                let remote_info_vec: Vec<RemoteInfo> = endpoint
                    .remote_info_iter()
                    .filter(|n| n.node_id != node_addr.node_id)
                    .collect::<Vec<_>>();

                for info in &remote_info_vec {
                    tracing::info!("{}", format_remote_info(info));
                }

                // tracing::info!(
                //     "🔍 {:?} Discovered Node: {:?}",
                //     endpoint.node_id().fmt_short(),
                //     node_addr,
                //     // remote_info_vec
                //     //     .iter()
                //     //     .map(|info| format!("{:?}", info)) // or use `to_string()` if `Display` is implemented
                //     //     .collect::<Vec<_>>()
                //     //     .join("\n\n")
                // );

                // if let Err(e) = erlang_sender_clone
                //     .send(ErlangMessageEvent {
                //         atom: atoms::iroh_gossip_node_discovered(),
                //         payload: vec![
                //             endpoint.node_id().fmt_short(),
                //             node_addr.node_id.fmt_short(),
                //             format!("{:?}", remote_info.latency)
                //         ],
                //     })
                //     .await
                // {
                //     tracing::warn!(
                //         "❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}",
                //         e
                //     );
                // }
            }
            Err(lagged) => {
                tracing::warn!(
                    "🚨 {:?} Discovery stream lagged! Some items may have been lost.{:?}",
                    endpoint.node_id().fmt_short(),
                    lagged
                );
            }
        }
    }
}


fn format_remote_info(info: &RemoteInfo) -> String {
    let mut out = String::new();

    use std::fmt::Write;

    writeln!(out, "🧩 Node: {}", info.node_id.fmt_short()).ok();
    writeln!(out, "  Relay: {}", info.relay_url.as_ref().map(|r| r.relay_url.to_string()).unwrap_or("None".into())).ok();
    writeln!(out, "  Conn Type: {:?}", info.conn_type);
    writeln!(out, "  Latency: {:?}", info.latency);
    writeln!(out, "  Addresses:");

    for addr in &info.addrs {
        writeln!(out, "    - {}", addr.addr).ok();
        if let Some(lat) = addr.latency {
            writeln!(out, "      ↳ Latency: {:?}", lat).ok();
        } else {
            writeln!(out, "      ↳ Latency: N/A").ok();
        }

        writeln!(out, "      ↳ Sources:").ok();
        for (src, age) in &addr.sources {
            writeln!(out, "          • {}: {:?}", source_display_name(src), age).ok();
        }
    }
    out
}

fn source_display_name(src: &Source) -> String {
    match src {
        Source::Discovery { name } => format!("Discovery ({})", name),
        Source::NamedApp { name } => format!("NamedApp ({})", name),
        Source::Saved => "Saved".to_string(),
        Source::Udp => "Udp".to_string(),
        Source::Relay => "Relay".to_string(),
        Source::App => "App".to_string(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Ticket {
    topic: TopicId,
    nodes: Vec<NodeAddr>,
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
    rustler::resource!(NodeRef, env);
    println!("Rust NIF Iroh module loaded successfully.");

    // start_continuous_flamegraph(180);
    // start_deadlock_checker();

    true
}

rustler::init!("Elixir.IrohEx.Native", load = on_load);
