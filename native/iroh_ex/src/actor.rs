use std::fmt::Debug;
use std::sync::Arc;

use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Mutex;

/// A basic actor trait that defines the behavior for message handling
#[async_trait::async_trait]
pub trait Actor: Send + Sync + 'static {
    /// The type of messages this actor can handle
    type Message: Send + 'static;
    /// The type of errors that can occur during message handling
    type Error: Debug + Send + Sync + 'static;

    /// Handle a message sent to this actor
    async fn handle(&mut self, msg: Self::Message) -> Result<(), Self::Error>;
}

/// A handle to an actor that can be used to send messages
#[derive(Debug, Clone)]
pub struct ActorHandle<M> {
    sender: Sender<M>,
}

impl<M: Send + 'static> ActorHandle<M> {
    /// Create a new actor handle with the given sender
    pub fn new(sender: Sender<M>) -> Self {
        Self { sender }
    }

    /// Send a message to the actor
    pub async fn send(&self, msg: M) -> Result<(), mpsc::error::SendError<M>> {
        self.sender.send(msg).await
    }
}

/// Spawn a new actor with the given initial state
pub async fn spawn<A, M>(mut actor: A, buffer: usize) -> ActorHandle<M>
where
    A: Actor<Message = M> + Send + 'static,
    M: Send + 'static,
{
    let (sender, mut receiver) = mpsc::channel(buffer);
    let handle = ActorHandle::new(sender);

    tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            if let Err(e) = actor.handle(msg).await {
                tracing::error!("Actor error: {:?}", e);
            }
        }
    });

    handle
}
