#![no_main]
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(deprecated)]
#![allow(unused_must_use)]
#![allow(non_local_definitions)]
// #![allow(unexpected_cfgs)]
// #[cfg(not(clippy))]
// #[rustler::nif(schedule = "DirtyCpu")]

use iroh::endpoint;
use rustler::{Encoder, Env, Error as RustlerError, LocalPid, OwnedEnv, ResourceArc, Term};

use atty::Stream;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::FmtSubscriber;

use rand::Rng;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout, Duration};

use std::fmt;
use std::str::FromStr;

use anyhow::Result;
use iroh::{
    endpoint::Connection,
    protocol::{ProtocolHandler, Router},
    Endpoint, NodeAddr, NodeId, SecretKey,
};

use quic_rpc::transport::flume::FlumeConnector;

pub(crate) type BlobsClient = iroh_blobs::rpc::client::blobs::Client<
    FlumeConnector<iroh_blobs::rpc::proto::Response, iroh_blobs::rpc::proto::Request>,
>;
pub(crate) type DocsClient = iroh_docs::rpc::client::docs::Client<
    FlumeConnector<iroh_docs::rpc::proto::Response, iroh_docs::rpc::proto::Request>,
>;

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

pub static RUNTIME: Lazy<Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

const ALPN: &[u8] = b"iroh-example/echo/0";

// const TOPIC_NAME: &str = "ehaaöskdjfasdjföasdjföa";

static TOPIC_NAME: Lazy<String> = Lazy::new(|| generate_topic_name());

fn generate_topic_name() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
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

#[rustler::nif]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

pub struct NodeRef(pub(crate) Arc<Mutex<NodeState>>);

pub struct NodeState {
    pub pid: LocalPid,
    pub endpoint: Endpoint,
    pub router: Router,
    pub gossip: Gossip,
    pub sender: GossipSender,
}

impl NodeState {
    pub fn new(
        pid: LocalPid,
        endpoint: Endpoint,
        router: Router,
        gossip: Gossip,
        sender: GossipSender,
    ) -> Self {
        NodeState {
            pid,
            endpoint,
            router,
            gossip,
            sender,
        }
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
    // let secret_key = SecretKey::generate(rand::rngs::OsRng);
    //println!("secret key: {secret_key}");
    let secret_key = "blabalba";
    Ok(secret_key.to_string())
}


#[rustler::nif(schedule = "DirtyCpu")]
pub fn create_node_async(env: Env, pid: LocalPid) -> Result<ResourceArc<NodeRef>, RustlerError> {
    // Block inside DirtyCpu to safely wait for the async task
    tokio::task::block_in_place(|| {
        RUNTIME.block_on(async { create_node_async_internal(pid).await })
    })
}

// The actual async function that creates a node
async fn create_node_async_internal(pid: LocalPid) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let endpoint = Endpoint::builder()
        .discovery_local_network()
        .bind()
        .await
        .map_err(|e| RustlerError::Term(Box::new(format!("Endpoint error: {}", e))))?;

    println!("Endpoint node id: {:?}", endpoint.node_id());

    let endpoint_clone = endpoint.clone();
    let mut builder = iroh::protocol::Router::builder(endpoint.clone());

    let gossip = Gossip::builder()
        .spawn(endpoint.clone())
        .await
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip protocol error: {}", e))))?;

    builder = builder.accept(GossipALPN, gossip.clone());
    builder = builder.accept(ALPN, Echo);

    let router = builder
        .spawn()
        .await
        .map_err(|e| RustlerError::Term(Box::new(format!("Router error: {}", e))))?;

    let router_clone = router.clone();

    let node_addr = router
        .endpoint()
        .node_addr()
        .await
        .map_err(|e| RustlerError::Term(Box::new(format!("Node addr error: {}", e))))?;

    let topic = gossip
        .subscribe(
            TopicId::from_bytes(string_to_32_byte_array(&TOPIC_NAME.to_string())),
            vec![],
        )
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    let (sender, _receiver) = topic.split();

    let state = NodeState::new(pid, endpoint_clone, router_clone, gossip, sender);
    let resource = ResourceArc::new(NodeRef(Arc::new(Mutex::new(state))));

    Ok(resource)
}

#[rustler::nif]
pub fn create_node(env: Env, pid: LocalPid) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let endpoint = RUNTIME
        .block_on(
            Endpoint::builder()
                .discovery_local_network()
                // .discovery_n0()
                .bind(),
        )
        .map_err(|e| RustlerError::Term(Box::new(format!("Endpoint error: {}", e))))?;

    println!("Endpoint node id: {:?}", endpoint.node_id());

    let endpoint_clone = endpoint.clone();

    let mut builder = iroh::protocol::Router::builder(endpoint.clone());

    let gossip = RUNTIME
        .block_on(Gossip::builder().spawn(endpoint.clone()))
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip protocol error: {}", e))))?;

    builder = builder.accept(GossipALPN, gossip.clone());
    builder = builder.accept(ALPN, Echo);

    let router = RUNTIME
        .block_on(builder.spawn())
        .map_err(|e| RustlerError::Term(Box::new(format!("Router error: {}", e))))?;

    let router_clone = router.clone();

    let node_addr = RUNTIME
        .block_on(router.endpoint().node_addr())
        .map_err(|e| RustlerError::Term(Box::new(format!("Node addr error: {}", e))))?;

    let node_ids: Vec<_> = vec![];

    // Subscribe to the topic.
    // Since the `node_ids` list is empty, we will
    // subscribe to the topic, but not attempt to
    // connect to any other nodes.

    let topic = RUNTIME
        .block_on(async {
            gossip.subscribe(
                TopicId::from_bytes(string_to_32_byte_array(&*TOPIC_NAME.to_string())),
                node_ids,
            )
        })
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    let (sender, _receiver) = topic.split();

    let state = NodeState::new(pid, endpoint_clone, router_clone, gossip, sender);
    let resource = ResourceArc::new(NodeRef(Arc::new(Mutex::new(state))));

    Ok(resource)
}

#[rustler::nif]
pub fn create_ticket(env: Env, node_ref: ResourceArc<NodeRef>) -> Result<String, RustlerError> {
    println!("Create ticket");

    let resource_arc = node_ref.0.clone();

    let endpoint = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.endpoint.clone()
    };

    let topic = TopicId::from_bytes(string_to_32_byte_array(&*TOPIC_NAME.to_string()));

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

#[rustler::nif]
pub fn send_message(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    message: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {
    println!("Message: {:?}", message);

    let resource_arc = node_ref.0.clone();

    let endpoint = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.endpoint.clone()
    };

    let gossip = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.gossip.clone()
    };

    let sender = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.sender.clone()
    };

    let message = Message::AboutMe {
        from: endpoint.node_id(),
        name: String::from(message),
    };

    RUNTIME
        .block_on(sender.broadcast(message.to_vec().into()))
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip broadcast error: {}", e))))?;

    Ok(node_ref)
}

#[rustler::nif]
pub fn connect_node(
    env: Env,
    node_ref: ResourceArc<NodeRef>,
    ticket: String,
) -> Result<ResourceArc<NodeRef>, RustlerError> {

    let node_ref_clone = node_ref.clone();
    RUNTIME.spawn(async move {
        if let Err(e) = async_work(node_ref_clone, ticket).await {
            tracing::error!("❌ Error in async task: {:?}", e);
        }
    });

    // Return immediately, allowing Elixir's Task to execute in parallel
    Ok(node_ref)
}


async fn async_work(node_ref: ResourceArc<NodeRef>, ticket: String) -> Result<(), RustlerError> {
    let resource_arc = node_ref.0.clone();

    let Ticket { topic, nodes } = Ticket::from_str(&ticket)
        .map_err(|e| RustlerError::Term(Box::new(format!("❌ Ticket parsing error: {}", e))))?;

    let endpoint_conn = {
        let endpoint_conn = resource_arc.lock().unwrap();
        endpoint_conn.endpoint.clone()
    };

    let gossip = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.gossip.clone()
    };

    tracing::info!("connect_node endpoint_Ptr:{:?} topic: {:?} nodes: {:?}", &endpoint_conn as *const _, topic, nodes);

    let node_ids: Vec<_> = nodes.iter().map(|p| p.node_id).collect();
    if nodes.is_empty() {
        tracing::info!("> Waiting for nodes...");
    } else {
        for node in nodes {
            endpoint_conn.add_node_addr(node).map_err(|e| {
                RustlerError::Term(Box::new(format!("❌ Failed to add node: {}", e)))
            })?;
        }
    }

    // 🛠 **Fix: Correctly handle the timeout and subscription errors**
    let topic_result = timeout(Duration::from_secs(5), gossip.subscribe_and_join(topic, node_ids)).await;

    let topic = match topic_result {
        Err(_) => {
            return Err(RustlerError::Term(Box::new("⏳ Gossip timeout".to_string())));
        }
        Ok(Err(e)) => {
            return Err(RustlerError::Term(Box::new(format!("❌ Gossip subscribe error: {:?}", e))));
        }
        Ok(Ok(topic)) => topic,
    };

    let (sender, mut receiver) = topic.split();
    let (mpsc_event_sender, mpsc_event_receiver) = mpsc::channel::<Event>(100);
    let mpsc_event_receiver = Arc::new(RwLock::new(mpsc_event_receiver));
    let mpsc_event_receiver_clone = mpsc_event_receiver.clone();

    // 🔄 **Gossip Event Listener**
    RUNTIME.spawn(async move {
        tracing::info!("🎧 Listening for gossip events...");
        loop {
            match receiver.next().await {
                Some(Ok(event)) => {
                    tracing::debug!("🔔 Gossip Event: {:?}", event);
                    if let Err(e) = mpsc_event_sender.send(event).await {
                        tracing::warn!("⚠️ Failed to forward event: {:?}", e);
                    }
                }
                Some(Err(e)) => {
                    tracing::error!("❌ Error receiving gossip event: {:?}", e);
                    tracing::info!("🔄 Restarting event handler in 2s...");
                    sleep(Duration::from_secs(2)).await;
                }
                None => {
                    tracing::warn!("⚠️ Gossip event stream ended. Reconnecting...");
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });

    // 🔄 **Message Processing Loop**
    RUNTIME.spawn(async move {
        let mut names = HashMap::new();
        let mut receiver = mpsc_event_receiver_clone.write().await;

        while let Some(event) = receiver.recv().await {
            tracing::debug!("📩 Received event: {:?}", event);

            if let Event::Gossip(GossipEvent::Received(msg)) = event {
                match Message::from_bytes(&msg.content) {
                    Ok(message) => {
                        match message {
                            Message::AboutMe { from, name } => {
                                names.insert(from, name.clone());
                                tracing::info!("💬 {} MSG {}", from.fmt_short(), name);
                            }
                            Message::Message { from, text } => {
                                let name = names.get(&from).map_or_else(|| from.fmt_short(), String::to_string);
                                tracing::info!("📝 {}: {}", name, text);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("❌ Failed to parse message: {:?}", e);
                    }
                }
            }
        }
    });

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


async fn log_discovery_stream(endpoint: Endpoint) {
    let mut stream = endpoint.discovery_stream();

    while let Some(result) = stream.next().await {
        match result {
            Ok(node_addr) => {
                tracing::info!("🔍 Discovered Node: {:?}", node_addr);
            }
            Err(lagged) => {
                tracing::warn!("🚨 Discovery stream lagged! Some items may have been lost.{:?}", lagged);
            }
        }
    }
}

//async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
    // let receiver_guard = receiver.read().await;

    println!("Initialize subscribe loop");

    // keep track of the mapping between `NodeId`s and names
    let mut names = HashMap::new();
    // iterate over all events
    while let Some(event) = receiver.try_next().await? {
        // println!("Received event {:?}", event);
        tracing::info!("Received event {:?}", event);
        // if the Event is a `GossipEvent::Received`, let's deserialize the message:
        if let Event::Gossip(GossipEvent::Received(msg)) = event {
            // deserialize the message and match on the
            // message type:
            match Message::from_bytes(&msg.content)? {
                Message::AboutMe { from, name } => {
                    // if it's an `AboutMe` message
                    // add and entry into the map
                    // and print the name
                    names.insert(from, name.clone());
                    tracing::info!("> {} is now known as {}", from.fmt_short(), name);
                }
                Message::Message { from, text } => {
                    // if it's a `Message` message,
                    // get the name from the map
                    // and print the message
                    let name = names
                        .get(&from)
                        .map_or_else(|| from.fmt_short(), String::to_string);
                    tracing::info!("{}: {}", name, text);
                }
            }
        }
    }
    Ok(())
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

// Rustler init

fn on_load(env: Env, _info: Term) -> bool {
    // check if pretty terminal or log file
    let is_tty = atty::is(Stream::Stdout);

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::new("iroh=error,iroh_ex=info")) // Enable DEBUG for `iroh_ex`
        .with_ansi(is_tty)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set up logging");

    tracing::debug!("Debug message from main!");

    println!("Initializing Rust Iroh NIF module ...");
    rustler::resource!(NodeRef, env);
    println!("Rust NIF Iroh module loaded successfully.");
    true
}

rustler::init!("Elixir.IrohEx.Native", load = on_load);
