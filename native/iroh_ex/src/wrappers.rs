use crate::state::ErlangMessageEvent;
use std::ptr;
use tokio::sync::mpsc;

use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver},
    proto::TopicId,
    ALPN as GossipALPN,
};

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
