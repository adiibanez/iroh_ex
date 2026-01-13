// #![allow(unused_imports)]
// #![allow(dead_code)]
// #![allow(unused_variables)]
// #![allow(deprecated)]
// #![allow(unused_must_use)]
// #![allow(non_local_definitions)]
// #![allow(unexpected_cfgs)]
// #[cfg(not(clippy))]
// #![allow(clippy::too_many_arguments)]

// use pprof::ProfilerGuard;

// use crate::actor::ActorHandle;
// use crate::gossip_actor::GossipActorMessage;
use iroh::{
    endpoint::Connection,
    protocol::{ProtocolHandler, Router},
    Endpoint, PublicKey, SecretKey,
};

// NodeId is now just PublicKey in iroh 0.95+
#[allow(dead_code)]
type NodeId = PublicKey;

use iroh_gossip::api::{GossipReceiver, GossipSender};
use iroh_gossip::net::Gossip;

// Blobs and Docs imports
use iroh_blobs::store::mem::MemStore as BlobMemStore;
use iroh_docs::protocol::Docs;
use rustler::{Atom, Encoder, Env, LocalPid, Monitor, OwnedEnv, Term};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;

// use std::sync::mpmc::Sender;
// use std::sync::mpmc::Receiver;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;

// Automerge imports
use automerge::AutoCommit;
use automerge::ActorId;

// use tracing_subscriber::{Registry, prelude::*};
// use console_subscriber::ConsoleLayer;

// use quic_rpc::transport::flume::FlumeConnector;

// pub(crate) type BlobsClient = iroh_blobs::rpc::client::blobs::Client<
//     FlumeConnector<iroh_blobs::rpc::proto::Response, iroh_blobs::rpc::proto::Request>,
// >;
// pub(crate) type DocsClient = iroh_docs::rpc::client::docs::Client<
//     FlumeConnector<iroh_docs::rpc::proto::Response, iroh_docs::rpc::proto::Request>,
// >;

// use iroh_gossip::{net::Gossip, ALPN as GossipALPN};
// use iroh_gossip::proto::TopicId;

use crate::tokio_runtime::RUNTIME;

// use parking_lot::deadlock;

// pub static RUNTIME: Lazy<Runtime> =
//     Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

// const topic_bytes = rand::random();
// static topic_bytes: [u8; 32] = rand::random();

// pub static TOPIC_BYTES: Lazy<[u8; 32]> =
//     Lazy::new(|| rand::random<[u8; 32]>().expect("Failed to create topic random bytes"));

pub struct NodeRef(pub(crate) Arc<Mutex<NodeState>>);

pub struct NodeState {
    pub pid: LocalPid,
    pub monitor_ref: Option<Monitor>,
    pub endpoint: Endpoint,
    pub router: Router,
    pub gossip: Gossip,
    pub sender: GossipSender,
    pub receiver: GossipReceiver,
    pub mpsc_event_sender: Sender<ErlangMessageEvent>,
    pub mpsc_event_receiver: Arc<RwLock<mpsc::Receiver<ErlangMessageEvent>>>,
    pub erlang_event_handler_task: Option<JoinHandle<()>>,
    pub event_handler_task: Option<JoinHandle<()>>,
    pub discovery_event_handler_task: Option<JoinHandle<()>>,
    // Blobs and Docs
    pub blobs_store: Option<BlobMemStore>,
    pub docs: Option<Docs>,
    // Automerge CRDT support
    pub automerge_docs: HashMap<String, AutoCommit>,
    pub automerge_actor: Option<ActorId>,
    pub automerge_sync_states: HashMap<String, HashMap<String, automerge::sync::State>>,
}

impl NodeState {
    pub fn new(
        pid: LocalPid,
        endpoint: Endpoint,
        router: Router,
        gossip: Gossip,
        sender: GossipSender,
        receiver: GossipReceiver,
        mpsc_event_sender: Sender<ErlangMessageEvent>,
        mpsc_event_receiver: Arc<RwLock<mpsc::Receiver<ErlangMessageEvent>>>,
        blobs_store: Option<BlobMemStore>,
        docs: Option<Docs>,
    ) -> Self {
        // Generate actor ID from endpoint public key for deterministic identity
        let actor_id = Some(ActorId::from(endpoint.id().as_bytes()));

        NodeState {
            pid,
            monitor_ref: None,
            endpoint,
            router,
            gossip,
            sender,
            receiver,
            mpsc_event_sender,
            mpsc_event_receiver,
            erlang_event_handler_task: None,
            event_handler_task: None,
            discovery_event_handler_task: None,
            blobs_store,
            docs,
            // Automerge fields
            automerge_docs: HashMap::new(),
            automerge_actor: actor_id,
            automerge_sync_states: HashMap::new(),
        }
    }
}

impl Drop for NodeState {
    fn drop(&mut self) {
        tracing::debug!("🚀 Cleaning up NodeState before exit!");

        if let Some(handle) = self.erlang_event_handler_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.event_handler_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.discovery_event_handler_task.take() {
            handle.abort();
        }

        let gossip = self.gossip.clone();
        let router = self.router.clone();
        let endpoint = self.endpoint.clone();
        let mpsc_event_sender = self.mpsc_event_sender.clone();
        let monitor_ref = self.monitor_ref;

        RUNTIME.spawn(async move {
            let _ = gossip.shutdown().await;
            let _ = router.shutdown().await;
            endpoint.close().await;
            tracing::debug!("✅ NodeState cleanup complete!");
        });
    }
}

#[derive(Debug, Clone)]
pub enum Payload {
    String(String),
    Binary(Vec<u8>),
    Tuple(Vec<Payload>),
    Map(Vec<(Payload, Payload)>),
    List(Vec<Payload>),
    Integer(i64),
    Float(f64),
}

impl Payload {
    pub fn encode<'a>(&self, env: Env<'a>) -> Term<'a> {
        match self {
            Payload::String(s) => s.encode(env),
            Payload::Binary(b) => b.encode(env),
            Payload::Tuple(t) => {
                let terms: Vec<Term> = t.iter().map(|p| p.encode(env)).collect();
                terms.encode(env)
            }
            Payload::Map(m) => {
                let mut map = Term::map_new(env);
                for (k, v) in m {
                    map = map.map_put(k.encode(env), v.encode(env)).unwrap();
                }
                map
            }
            Payload::List(l) => {
                let terms: Vec<Term> = l.iter().map(|p| p.encode(env)).collect();
                terms.encode(env)
            }
            Payload::Integer(i) => i.encode(env),
            Payload::Float(f) => f.encode(env),
        }
    }

    pub fn from_map(map: HashMap<String, String>) -> Self {
        let payload_map: Vec<(Payload, Payload)> = map
            .into_iter()
            .map(|(k, v)| (Payload::String(k), Payload::String(v)))
            .collect();
        Payload::Map(payload_map)
    }
}

#[derive(Debug)]
pub struct ErlangMessageEvent {
    pub atom: Atom,
    pub payload: Payload,
}

pub mod atoms {
    rustler::atoms! {
        ok,
        error,
        // fake async alive message
        ping,

        // errors
        lock_fail,
        not_found,
        offer_error,
        candidate_error,

        // TODO: rename non gossip related events to _node_ in both rust / elixir
        iroh_node_connected,
        iroh_node_test,
        iroh_gossip_joined,
        iroh_gossip_neighbor_up,
        iroh_gossip_neighbor_down,
        iroh_gossip_node_discovered,
        iroh_gossip_message_received,
        iroh_gossip_message_unhandled,
        iroh_gossip_list_topics,

        // Automerge events
        automerge_doc_created,
        automerge_doc_changed,
        automerge_doc_synced,
        automerge_doc_merged,
        automerge_sync_message_received,
        automerge_conflict_detected,
    }
}
