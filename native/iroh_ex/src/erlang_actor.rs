use crate::actor::Actor;
use async_trait::async_trait;
use rustler::{Encoder, LocalPid, OwnedEnv, Term};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::state::atoms;
use crate::state::ErlangMessageEvent;
use crate::state::Payload;

// 1. Define the message types for the actor
#[derive(Debug)]
enum ErlangEventHandlerMessage {
    SendEvent(ErlangMessageEvent),
    CheckAlive,
    Shutdown,
}

// 2. Create the actor struct
struct ErlangEventHandlerActor {
    pid: LocalPid,
    env: Arc<Mutex<OwnedEnv>>,
    is_alive: Arc<AtomicBool>,
    last_check: Instant,
    check_interval: Duration,
}

impl ErlangEventHandlerActor {
    fn new(pid: LocalPid, check_interval: Duration) -> Self {
        Self {
            pid,
            env: Arc::new(Mutex::new(OwnedEnv::new())),
            is_alive: Arc::new(AtomicBool::new(true)),
            last_check: Instant::now(),
            check_interval,
        }
    }

    async fn check_alive(&mut self) -> bool {
        if self.last_check.elapsed() >= self.check_interval {
            // Try to send a ping message
            let is_alive = self
                .env
                .lock()
                .unwrap()
                .send_and_clear(&self.pid, |env| atoms::ping().encode(env))
                .is_ok();

            self.is_alive.store(is_alive, Ordering::SeqCst);
            self.last_check = Instant::now();
            is_alive
        } else {
            self.is_alive.load(Ordering::SeqCst)
        }
    }

    async fn send_event(&mut self, event: ErlangMessageEvent) -> Result<(), anyhow::Error> {
        if !self.check_alive().await {
            return Err(anyhow::anyhow!("Elixir process is not alive"));
        }
        let mut env = self.env.lock().unwrap();
        env.send_and_clear(&self.pid, |env| {
            let terms: Vec<Term> = match &event.payload {
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
                }
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
        })
        .map_err(|e| anyhow::anyhow!("Failed to send message: {:?}", e))
    }
}

// 3. Implement the Actor trait
#[async_trait]
impl Actor for ErlangEventHandlerActor {
    type Message = ErlangEventHandlerMessage;
    type Error = anyhow::Error;

    async fn handle(&mut self, msg: Self::Message) -> Result<(), Self::Error> {
        match msg {
            ErlangEventHandlerMessage::SendEvent(event) => {
                self.send_event(event).await?;
            }
            ErlangEventHandlerMessage::CheckAlive => {
                self.check_alive().await;
            }
            ErlangEventHandlerMessage::Shutdown => {
                // Clean shutdown
                self.is_alive.store(false, Ordering::SeqCst);
            }
        }
        Ok(())
    }
}

// 4. Create the actor handle
struct ErlangEventHandlerHandle {
    sender: mpsc::Sender<ErlangEventHandlerMessage>,
}

impl ErlangEventHandlerHandle {
    fn new(mut actor: ErlangEventHandlerActor) -> Self {
        let (sender, mut receiver) = mpsc::channel(32);

        tokio::spawn(async move {
            while let Some(msg) = receiver.recv().await {
                if let Err(e) = actor.handle(msg).await {
                    tracing::error!("Erlang event handler error: {:?}", e);
                }
            }
        });

        Self { sender }
    }

    async fn send(
        &self,
        msg: ErlangEventHandlerMessage,
    ) -> Result<(), mpsc::error::SendError<ErlangEventHandlerMessage>> {
        self.sender.send(msg).await
    }
}
