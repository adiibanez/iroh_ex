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

// use pprof::ProfilerGuard;

use chrono::Local;
use iroh::endpoint;
use iroh::RelayUrl;
use rustler::{
    Encoder, Env, Error as RustlerError, LocalPid, Monitor, NifResult, OwnedEnv, ResourceArc, Term,
};

use atty::Stream;
use tokio::task::JoinHandle;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::FmtSubscriber;

use rand::Rng;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::collections::HashSet;
// use std::sync::mpmc::Sender;
// use std::sync::mpmc::Receiver;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout, Duration};

// use tracing_subscriber::{Registry, prelude::*};
// use console_subscriber::ConsoleLayer;

use std::fmt;
use std::ptr;
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::{ProtocolHandler, Router},
    Endpoint, NodeAddr, NodeId, SecretKey,
};

use rand::rngs::OsRng;

// use quic_rpc::transport::flume::FlumeConnector;

// pub(crate) type BlobsClient = iroh_blobs::rpc::client::blobs::Client<
//     FlumeConnector<iroh_blobs::rpc::proto::Response, iroh_blobs::rpc::proto::Request>,
// >;
// pub(crate) type DocsClient = iroh_docs::rpc::client::docs::Client<
//     FlumeConnector<iroh_docs::rpc::proto::Response, iroh_docs::rpc::proto::Request>,
// >;

// use iroh_gossip::{net::Gossip, ALPN as GossipALPN};
// use iroh_gossip::proto::TopicId;

use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver, GossipSender},
    proto::TopicId,
    ALPN as GossipALPN,
};

use iroh::discovery::DiscoveryItem;

use serde::{Deserialize, Serialize};

use n0_future::boxed::BoxFuture;
use n0_future::StreamExt;

use rand::distributions::Alphanumeric;


use std::{
    fs::File,
    path::PathBuf,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::deadlock;


pub static RUNTIME: Lazy<Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

const ALPN: &[u8] = b"iroh-example/echo/0";

// const TOPIC_NAME: &str = "ehaaöskdjfasdjföasdjföa";

static TOPIC_NAME: Lazy<String> = Lazy::new(generate_topic_name);

fn generate_topic_name() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from) // Convert u8 to char
        .collect()
}

// const topic_bytes = rand::random();
// static topic_bytes: [u8; 32] = rand::random();

// pub static TOPIC_BYTES: Lazy<[u8; 32]> =
//     Lazy::new(|| rand::random<[u8; 32]>().expect("Failed to create topic random bytes"));

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

pub struct NodeRef(pub(crate) Arc<Mutex<NodeState>>);

pub struct NodeState {
    pub pid: LocalPid,
    pub monitor_ref: Option<Monitor>,
    pub endpoint: Endpoint,
    pub router: Router,
    pub gossip: Gossip,
    pub sender: GossipSender,
    pub mpsc_event_sender: Sender<ErlangMessageEvent>,
    pub mpsc_event_receiver: Arc<RwLock<mpsc::Receiver<ErlangMessageEvent>>>,
    pub erlang_event_handler_task: Option<JoinHandle<()>>,
    pub event_handler_task: Option<JoinHandle<()>>,
    pub discovery_event_handler_task: Option<JoinHandle<()>>,
}

impl NodeState {
    pub fn new(
        pid: LocalPid,
        endpoint: Endpoint,
        router: Router,
        gossip: Gossip,
        sender: GossipSender,
        mpsc_event_sender: Sender<ErlangMessageEvent>,
        mpsc_event_receiver: Arc<RwLock<mpsc::Receiver<ErlangMessageEvent>>>,
    ) -> Self {
        NodeState {
            pid,
            monitor_ref: None,
            endpoint,
            router,
            gossip,
            sender,
            mpsc_event_sender,
            mpsc_event_receiver,
            erlang_event_handler_task: None,
            event_handler_task: None,
            discovery_event_handler_task: None,
            // mpsc_event_receiver,
        }
    }
}

impl Drop for NodeState {
    fn drop(&mut self) {
        // tracing::info!("🚀 Cleaning up NodeState before exit!");
        let gossip = self.gossip.clone();
        let router = self.router.clone();
        let endpoint = self.endpoint.clone();
        let mpsc_event_sender = self.mpsc_event_sender.clone();
        let monitor_ref = self.monitor_ref;

        RUNTIME.spawn(async move {
            gossip.shutdown().await;
            router.shutdown();
            endpoint.close().await;
            tracing::debug!("✅ NodeState cleanup complete!");
        });
    }
}

struct GossipWrapper {
    gossip: iroh_gossip::net::Gossip,
}

impl Drop for GossipWrapper {
    fn drop(&mut self) {
        tracing::debug!(
            "🚀 GossipWrapper: Gossip {:?} at address {:p} is being dropped!",
            self.gossip,
            ptr::addr_of!(self)
        );
    }
}
struct EndpointWrapper {
    endpoint: iroh::Endpoint,
}

impl Drop for EndpointWrapper {
    fn drop(&mut self) {
        tracing::debug!(
            "🚀 EndpointWrapper: Endpoint {:?}  at address {:p} is being dropped!",
            self.endpoint.node_id().fmt_short(),
            ptr::addr_of!(self)
        );
    }
}
struct GossipReceiverWrapper {
    receiver: GossipReceiver,
}

impl Drop for GossipReceiverWrapper {
    fn drop(&mut self) {
        tracing::debug!(
            "🚀 GossipReceiverWrapper: GossipReceiver {:?} at address {:p} is being dropped!",
            self.receiver,
            ptr::addr_of!(self)
        );
    }
}

struct MpscReceiverWrapper {
    receiver: mpsc::Receiver<ErlangMessageEvent>,
}

impl Drop for MpscReceiverWrapper {
    fn drop(&mut self) {
        tracing::debug!(
            "🚀 EndpointWrapper: Endpoint {:?} is being dropped!",
            self.receiver
        );
    }
}

#[derive(Debug)]
pub struct ErlangMessageEvent {
    atom: rustler::Atom,
    payload: Vec<String>,
}

mod atoms {
    rustler::atoms! {
        ok,
        error,

        // errors
        lock_fail,
        not_found,
        offer_error,
        candidate_error,

        // TODO: rename non gossip related events to _node_ in both rust / elixir
        iroh_node_connected,
        iroh_gossip_joined,
        iroh_gossip_neighbor_up,
        iroh_gossip_neighbor_down,
        iroh_gossip_node_discovered,
        iroh_gossip_message_received,
        iroh_gossip_message_unhandled,
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
    let mut rng = OsRng; // Create an instance of OsRng
    let secret_key = SecretKey::generate(&mut rng);

    // let secret_key = SecretKey::generate(OsRng);
    //println!("secret key: {secret_key}");
    // let secret_key = "blabalba";
    Ok(secret_key.to_string())
}

#[rustler::nif(schedule = "DirtyCpu")]
pub fn create_node(env: Env, pid: LocalPid) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let env_pid_clone = env.pid();
    let monitor_pid = pid;
    let topic_name = TOPIC_NAME.to_string();

    let (resource, monitor_ref) = RUNTIME.block_on(async move {
        // --- Start of async logic ---
        let endpoint = Endpoint::builder()
            // .discovery_n0()
            .discovery_local_network()
            .bind()
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Endpoint error: {}", e))))?;

        let endpoint_clone = endpoint.clone();
        let mut builder = iroh::protocol::Router::builder(endpoint.clone());

        let gossip = Gossip::builder()
            .spawn(endpoint.clone())
            .await
            .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {}", e))))?;

        builder = builder.accept(GossipALPN, gossip.clone());
        builder = builder.accept(ALPN, Echo);

        let router = builder
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
                    let terms: Vec<Term> =
                        event.payload.iter().map(|s| s.encode(env)).collect();

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
    let endpoint = {
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

    let (endpoint_conn, gossip, node_id, node_id_short, erlang_sender_clone) = {
        let state = resource_arc.lock().unwrap();
        (
            state.endpoint.clone(),
            state.gossip.clone(),
            state.endpoint.node_id(),
            state.endpoint.node_id().fmt_short(),
            state.mpsc_event_sender.clone(),
        )
    };

    let endpoint_clone = endpoint_conn.clone();

    let node_ref_clone = node_ref.clone();

    {
        let state = node_ref.0.lock().unwrap(); // Locks the mutex
        let pid = state.pid; // Clone only what is needed
        drop(state); // Explicitly drop the lock to avoid Send issues

        // Now that state is unlocked, we can create the task safely
        // RUNTIME.spawn(log_discovery_stream(node_ref_clone.clone(), pid))
    };

    // Re-lock the state and store the handle safely
    {
        let state = node_ref.0.lock().unwrap();
        // state.discovery_event_handler_task = Some(discovery_task);
    }

    tracing::debug!(
        "connect_node endpoint_Ptr:{:?} topic: {:?} nodes: {:?}",
        &endpoint_conn as *const _,
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
            payload: vec![node_id.fmt_short(), relay_url.as_str().to_string()],
        })
        .await
    {
        tracing::warn!(
            "❌ GossipEvent::Joined Failed to send erlang message: {:?}",
            e
        );
    }

    // if let Err(e) = msg_env.send_and_clear(&pid, |env| {
    //     (
    //         atoms::ok(),
    //         "Hello from connect_node_async_internal before inner",
    //     )
    //         .encode(env)
    // }) {
    //     tracing::debug!("⚠️ Failed to send unhandled message: {:?}", e);
    // }

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
                                    payload: vec![pub_keys
                                        .iter()
                                        .map(|pk| pk.fmt_short())
                                        .collect::<Vec<_>>()
                                        .join(",")],
                                })
                                .await
                            {
                                tracing::warn!("❌ GossipEvent::Joined Failed to send erlang message: {:?}", e);
                            }
                        }
                        Event::Gossip(GossipEvent::NeighborUp(pub_key)) => {
                            tracing::debug!("NeighborUp {:?}", pub_key);

                            if let Err(e) = erlang_sender_clone
                                .send(ErlangMessageEvent {
                                    atom: atoms::iroh_gossip_neighbor_up(),
                                    payload: vec![
                                        node_id_short_clone.clone(),
                                        pub_key.clone().fmt_short(),
                                    ],
                                })
                                .await
                            {
                                tracing::warn!("❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}", e);
                            }
                        }
                        Event::Gossip(GossipEvent::NeighborDown(pub_key)) => {
                            tracing::debug!("NeighborDown {:?}", pub_key);

                            if let Err(e) = erlang_sender_clone
                                .send(ErlangMessageEvent {
                                    atom: atoms::iroh_gossip_neighbor_down(),
                                    payload: vec![
                                        node_id_short_clone.clone(),
                                        pub_key.clone().fmt_short(),
                                    ],
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
                                                payload: vec![
                                                    node_id_short_clone.clone(),
                                                    name.clone(),
                                                ],
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
                                    payload: vec![message],
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

#[rustler::nif(schedule = "DirtyCpu")]
fn disconnect_node(node_ref: ResourceArc<NodeRef>) -> NifResult<()> {
    // let node = node_ref.lock().unwrap();
    // node.disconnect_all();  // Assuming an API to disconnect all peers
    Ok(())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn list_peers(node_ref: ResourceArc<NodeRef>) -> NifResult<Vec<String>> {
    let node = node_ref.0.clone();

    let endpoint = {
        let state = node_ref.0.lock().unwrap();
        state.endpoint.clone()
    };

    //endpoint.discovery_stream();

    let peers: Vec<_> = vec![]; //node.peers().iter().map(|p| p.to_string()).collect();
    Ok(peers)
}



#[rustler::nif(schedule = "DirtyCpu")]
pub fn cleanup(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
) -> NifResult<()> {
    let node_ref_clone = node_ref.clone();

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
            state.endpoint.clone(),
            state.mpsc_event_sender.clone(),
            state.endpoint.discovery_stream(),
        )
    };

    // let endpoint = state_clone.endpoint.clone();
    // let erlang_sender_clone = state_clone.mpsc_event_sender.clone();

    // let mut stream = endpoint.discovery_stream();
    let msg_env = OwnedEnv::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(node_addr) => {
                tracing::debug!(
                    "🔍 {:?} Discovered Node: {:?}",
                    endpoint.node_id().fmt_short(),
                    node_addr
                );

                if let Err(e) = erlang_sender_clone
                    .send(ErlangMessageEvent {
                        atom: atoms::iroh_gossip_node_discovered(),
                        payload: vec![
                            endpoint.node_id().fmt_short(),
                            node_addr.node_id().fmt_short(),
                        ],
                    })
                    .await
                {
                    tracing::warn!(
                        "❌ GossipEvent::NeighborUp Failed to send erlang message: {:?}",
                        e
                    );
                }
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
//             let guard = match ProfilerGuard::new(100) {
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

    tracing::debug!("Debug message from iroh_ex!");

    println!("Initializing Rust Iroh NIF module ...");
    rustler::resource!(NodeRef, env);
    println!("Rust NIF Iroh module loaded successfully.");

    // start_continuous_flamegraph(180);
    // start_deadlock_checker();

    true
}

rustler::init!("Elixir.IrohEx.Native", load = on_load);
