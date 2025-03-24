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

use tracing_subscriber::EnvFilter;
use tracing_subscriber::FmtSubscriber;

use rand::Rng;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

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

use serde::{Deserialize, Serialize};

use n0_future::boxed::BoxFuture;
use n0_future::StreamExt;

pub static RUNTIME: Lazy<Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

const ALPN: &[u8] = b"iroh-example/echo/0";

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
    // pub receiver: Arc<RwLock<GossipReceiver>>,
    // pub event_sender: mpsc::Sender<CentralEvent>,
    // pub event_receiver: Arc<RwLock<mpsc::Receiver<CentralEvent>>>,
    // pub discovered_peripherals: Arc<Mutex<HashMap<String, ResourceArc<PeripheralRef>>>>,
}

impl NodeState {
    pub fn new(
        pid: LocalPid,
        endpoint: Endpoint,
        router: Router,
        gossip: Gossip,
        sender: GossipSender,
        // receiver: Arc<RwLock<GossipReceiver>>
        // manager: Manager,
        // adapter: Adapter,
        // event_sender: mpsc::Sender<CentralEvent>,
        // event_receiver: Arc<RwLock<mpsc::Receiver<CentralEvent>>>,
    ) -> Self {
        NodeState {
            pid,
            endpoint,
            router,
            gossip,
            sender,
            // receiver
            // manager,
            // adapter,
            // event_sender,
            // event_receiver,
            // discovered_peripherals: Arc::new(Mutex::new(HashMap::new())),
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

#[rustler::nif]
pub fn create_node(env: Env, pid: LocalPid) -> Result<ResourceArc<NodeRef>, RustlerError> {
    let endpoint = RUNTIME
        .block_on(Endpoint::builder().discovery_n0().bind())
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

    let node_ids = vec![];

    // Subscribe to the topic.
    // Since the `node_ids` list is empty, we will
    // subscribe to the topic, but not attempt to
    // connect to any other nodes.

    // let topic =RUNTIME
    //     .block_on(gossip.subscribe(id, node_ids))
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Gossip subscribe error: {}", e))))?;

    let topic = RUNTIME
        .block_on(async {
            gossip.subscribe(
                TopicId::from_bytes(string_to_32_byte_array("test")),
                node_ids,
            )
        })
        .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    // let topic = gossip.subscribe(id, node_ids).map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    let (sender, receiver) = topic.split();

    // Broadcast a messsage to the topic.
    // Since no one else is apart of this topic,
    // this message is currently going out to no one.

    RUNTIME.spawn(subscribe_loop(receiver));

    // RUNTIME
    //     .block_on(sender.broadcast("sup".into()))
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Gossip broadcast error: {}", e))))?;

    // // let receiver_arc = Arc::new(RwLock::new(receiver));

    // let message = Message::AboutMe {
    //     from: endpoint.node_id(),
    //     name: String::from("alice"),
    // };

    // RUNTIME
    //     .block_on(sender.broadcast(message.to_vec().into()))
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Gossip broadcast error: {}", e))))?;

    // Open a connection to the accepting node
    // let conn = RUNTIME
    //     .block_on(endpoint_clone.connect(node_addr, ALPN))
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Conn error: {}", e))))?;

    // // let conn = endpoint.connect(addr, ALPN).await?;

    // // Open a bidirectional QUIC stream
    // let (mut send, mut recv) = RUNTIME
    //     .block_on(conn.open_bi())
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Conn open error: {}", e))))?;

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

    let topic = TopicId::from_bytes(string_to_32_byte_array("test"));

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
) -> Result<(), RustlerError> {
    let resource_arc = node_ref.0.clone();

    let Ticket { topic, nodes } = Ticket::from_str(&ticket.to_string())
        .map_err(|e| RustlerError::Term(Box::new(format!("Ticket parsing error: {}", e))))?;

    println!("connect_node: {:?} {:?}", topic, nodes);

    let endpoint_conn = {
        let endpoint_conn = resource_arc.lock().unwrap();
        endpoint_conn.endpoint.clone()
    };

    let endpoint_sender = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.sender.clone()
    };

    let gossip = {
        let endpoint = resource_arc.lock().unwrap();
        endpoint.gossip.clone()
    };

    let builder = Endpoint::builder();

    let node_ids: Vec<_> = nodes.iter().map(|p| p.node_id).collect();
    if nodes.is_empty() {
        println!("> waiting for nodes to join us...");
    } else {
        println!("> trying to connect to {} nodes...", nodes.len());
        // add the peer addrs from the ticket to our endpoint's addressbook so that they can be dialed
        for node in nodes.into_iter() {
            endpoint_conn.add_node_addr(node).map_err(|e| {
                RustlerError::Term(Box::new(format!("Ticket parsing error: {}", e)))
            })?;
        }
    };

    RUNTIME.spawn(async move {
        gossip.subscribe_and_join(topic, node_ids).await;
        println!("Connected!");
    });

    // let topic = RUNTIME
    //     .block_on(async { gossip.subscribe_and_join(topic, node_ids).await })
    //     // .block_on(async { gossip.subscribe(topic, node_ids) })
    //     .map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    // println!("Connected!");

    // // let topic = gossip.subscribe(id, node_ids).map_err(|e| RustlerError::Term(Box::new(format!("Gossip error: {:?}", e))))?;

    // let (sender, receiver) = topic.split();

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

//async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
    // let receiver_guard = receiver.read().await;

    println!("Initialize subscribe loop");

    // keep track of the mapping between `NodeId`s and names
    let mut names = HashMap::new();
    // iterate over all events
    while let Some(event) = receiver.try_next().await? {
        println!("Received event {:?}", event);
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
                    println!("> {} is now known as {}", from.fmt_short(), name);
                }
                Message::Message { from, text } => {
                    // if it's a `Message` message,
                    // get the name from the map
                    // and print the message
                    let name = names
                        .get(&from)
                        .map_or_else(|| from.fmt_short(), String::to_string);
                    println!("{}: {}", name, text);
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
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::new("iroh=info,iroh_ex=debug")) // Enable DEBUG for `iroh_ex`
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set up logging");

    tracing::debug!("Debug message from main!");

    println!("Initializing Rust Iroh NIF module ...");
    rustler::resource!(NodeRef, env);
    println!("Rust NIF Iroh module loaded successfully.");
    true
}

rustler::init!("Elixir.IrohEx.Native", load = on_load);
