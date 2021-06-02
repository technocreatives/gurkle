use std::sync::atomic::{AtomicU8, Ordering};
use std::{collections::HashMap, marker::PhantomData, pin::Pin, sync::Arc};

use async_stream::stream;
use futures_util::{stream::Stream, StreamExt};
use raw::{ClientMessage, GraphQLReceiver, GraphQLSender, ServerMessage};
use serde::{de::DeserializeOwned, Deserialize};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite};
use tracing::Instrument;

use tungstenite::error::ProtocolError;
pub use tungstenite::handshake::client::Request;
pub use tungstenite::Error;

use crate::ws::raw::MessageError;
use crate::RequestBody;

struct SubscriptionHandle {
    channel: mpsc::UnboundedSender<ServerMessage>,
    request: RequestBody,
    liveness: AtomicU8,
}

pub(crate) struct GraphQLWebSocket {
    subscriptions: Arc<RwLock<HashMap<u64, SubscriptionHandle>>>,
    connection: Arc<Mutex<GraphQLConnection>>,
    id_count: u64,
}

async fn process_server_message(
    msg: ServerMessage,
    subscriptions: &RwLock<HashMap<u64, SubscriptionHandle>>,
) {
    if let Some(id) = msg.id() {
        let guard = subscriptions.read().await;

        let requires_cleanup = if let Some(tx) = guard.get(&id) {
            let requires_cleanup = match &msg {
                ServerMessage::Error { .. } | ServerMessage::Complete { .. } => true,
                _ => false,
            };

            tx.channel.send(msg).is_err() || requires_cleanup
        } else {
            false
        };

        drop(guard);

        if requires_cleanup {
            let mut guard = subscriptions.write().await;
            guard.remove(&id);
        }
    } else {
        let guard = subscriptions.read().await;

        for tx in guard.values() {
            let _ = tx.channel.send(msg.clone());
        }
    };
}

struct GraphQLConnection {
    tx: mpsc::UnboundedSender<ClientMessage>,
    liveness: u8,
}

impl GraphQLConnection {
    async fn new<R: IntoClientRequest + Unpin>(
        request: R,
        subscriptions: Arc<RwLock<HashMap<u64, SubscriptionHandle>>>,
        liveness: u8,
    ) -> Result<(Self, JoinHandle<bool>), tungstenite::Error> {
        let (stream, _) = match connect_async(request).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let (sink, stream) = StreamExt::split(stream);
        let (tx_out, mut rx_out) = mpsc::unbounded_channel();

        let span = tracing::trace_span!("receiver");

        let subs0 = subscriptions.clone();
        let should_reconnect = tokio::spawn(
            async move {
                // let mut tx = tx_in0;
                let rx = GraphQLReceiver { stream };
                let subscriptions = subs0;

                let mut stream = rx.stream();
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(ServerMessage::ConnectionKeepAlive) => {}
                        Ok(v) => process_server_message(v, &*subscriptions).await,
                        Err(MessageError::WebSocket(tungstenite::Error::Protocol(
                            ProtocolError::ResetWithoutClosingHandshake,
                        ))) => {
                            return true;
                        }
                        Err(MessageError::WebSocket(tungstenite::Error::ConnectionClosed)) => {
                            // Not an error, websocket has closed normally.
                            break;
                        }
                        Err(e) => {
                            tracing::error!("Error handling next subscription message: {:?}", e)
                        }
                    }
                }
                return false;
            }
            .instrument(span),
        );

        let span = tracing::trace_span!("sender");
        let subs0 = subscriptions.clone();
        tokio::spawn(
            async move {
                let mut tx = GraphQLSender { sink };

                tx.send(ClientMessage::ConnectionInit { payload: None })
                    .await
                    .unwrap();

                // Iterate through existing subscriptions first, for reconnect situation
                let subs = subs0.read().await;
                for (id, handle) in subs.iter() {
                    if liveness != handle.liveness.load(Ordering::SeqCst) {
                        handle.liveness.store(liveness, Ordering::SeqCst);
                    } else {
                        continue;
                    }

                    match tx
                        .send(ClientMessage::Start {
                            id: id.to_string(),
                            payload: handle.request.clone(),
                        })
                        .await
                    {
                        Ok(()) => {}
                        Err(e) => tracing::error!("Error subscribing to id {}: {:?}", id, e),
                    }
                }

                while let Some(msg) = rx_out.recv().await {
                    match tx.send(msg).await {
                        Ok(()) => {}
                        Err(e) => tracing::error!("Error sending client message: {:?}", e),
                    }
                }
            }
            .instrument(span),
        );

        Ok((
            GraphQLConnection {
                tx: tx_out,
                liveness,
            },
            should_reconnect,
        ))
    }
}

fn spawn_reconnecter<R: IntoClientRequest + Clone + Unpin + Send + Sync + 'static>(
    should_reconnect: JoinHandle<bool>,
    connection: Arc<Mutex<GraphQLConnection>>,
    subscriptions: Arc<RwLock<HashMap<u64, SubscriptionHandle>>>,
    request: R,
    liveness: u8,
) {
    tokio::spawn(async move {
        let should_reconnect = should_reconnect.await.unwrap();

        if should_reconnect {
            let liveness = liveness.wrapping_add(1);
            let should_reconnect = loop {
                let (new_connection, should_reconnect) =
                    match GraphQLConnection::new(request.clone(), subscriptions.clone(), liveness)
                        .await
                    {
                        Ok(x) => x,
                        Err(e) => {
                            eprintln!("Reconnect error: {:?}", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            continue;
                        }
                    };
                {
                    let mut guard = connection.lock().await;
                    *guard = new_connection;
                }

                break should_reconnect;
            };

            spawn_reconnecter(
                should_reconnect,
                connection,
                subscriptions,
                request,
                liveness,
            );
        }
    });
}

impl GraphQLWebSocket {
    pub async fn connect<R: IntoClientRequest + Clone + Unpin + Send + Sync + 'static>(
        request: R,
    ) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let (connection, should_reconnect) =
            GraphQLConnection::new(request.clone(), subscriptions.clone(), 0).await?;
        let connection = Arc::new(Mutex::new(connection));

        spawn_reconnecter(
            should_reconnect,
            connection.clone(),
            subscriptions.clone(),
            request,
            0,
        );

        let socket = GraphQLWebSocket {
            subscriptions,
            connection,
            id_count: 0,
        };

        Ok(socket)
    }

    pub async fn subscribe<T>(&mut self, payload: RequestBody) -> Result<Subscription<T>, Error>
    where
        T: for<'de> Deserialize<'de> + Unpin + Send + 'static,
    {
        self.id_count += 1;
        let id = format!("{:X}", self.id_count);

        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut lock = self.subscriptions.write().await;
            lock.insert(
                self.id_count,
                SubscriptionHandle {
                    channel: tx,
                    request: payload.clone(),
                    liveness: AtomicU8::new(self.connection.lock().await.liveness),
                },
            );
        }

        tracing::trace!("Sending start message");
        let tx = {
            let id = id.clone();
            let guard = self.connection.lock().await;
            guard.tx.send(ClientMessage::Start { id, payload }).unwrap();
            guard.tx.clone()
        };
        tracing::trace!("Sent!");

        // TODO: check for errors here, so we can exit early.

        let sub = Subscription::<T>::new(id, tx, rx);
        Ok(sub)
    }
}

pub(crate) struct Subscription<
    T: for<'de> Deserialize<'de> = serde_json::Value,
    E: for<'de> Deserialize<'de> = serde_json::Value,
> {
    id: String,
    tx: mpsc::UnboundedSender<ClientMessage>,
    rx: mpsc::UnboundedReceiver<ServerMessage>,
    ty_value: PhantomData<T>,
    ty_error: PhantomData<E>,
}

impl<T, E> Subscription<T, E>
where
    T: DeserializeOwned + Unpin + Send + 'static,
    E: DeserializeOwned + Unpin + Send + 'static,
{
    pub fn new(
        id: String,
        tx: mpsc::UnboundedSender<ClientMessage>,
        rx: mpsc::UnboundedReceiver<ServerMessage>,
    ) -> Self {
        Self {
            id,
            tx,
            rx,
            ty_value: PhantomData,
            ty_error: PhantomData,
        }
    }

    fn spawn_task(self) -> mpsc::UnboundedReceiver<Result<serde_json::Value, crate::Error>> {
        let span = tracing::trace_span!("subscription", id = %self.id);
        let this = self;
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(
            async move {
                let mut this = this;

                while let Some(msg) = this.rx.recv().await {
                    tracing::trace!("{:?}", &msg);
                    match msg {
                        ServerMessage::Data { id, payload } => {
                            if id == this.id {
                                match tx.send(payload.into()) {
                                    Ok(_) => {
                                        tracing::trace!("Send payload successful.");
                                    }
                                    Err(e) => tracing::error!(
                                        "Error sending server message data: {:?}",
                                        e
                                    ),
                                }
                            } else {
                                tracing::error!(
                                    "Subscription is receiving invalid messages! Got: {}",
                                    id
                                );
                            }
                        }
                        ServerMessage::Complete { id } => {
                            if id == this.id {
                                return;
                            }
                        }
                        ServerMessage::ConnectionError { payload } => {
                            match tx.send(Err(crate::Error::Server(payload))) {
                                Ok(_) => {}
                                Err(e) => tracing::error!("Connection error: {:?}", e),
                            }
                            return;
                        }
                        ServerMessage::Error { id, payload } => {
                            if id == this.id {
                                match tx.send(Err(crate::Error::Server(payload))) {
                                    Ok(_) => {}
                                    Err(e) => tracing::error!("General error: {:?}", e),
                                }
                            }
                        }
                        ServerMessage::ConnectionAck => {}
                        ServerMessage::ConnectionKeepAlive => {}
                    }
                }
            }
            .instrument(span),
        );

        rx
    }

    pub fn stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = Result<serde_json::Value, crate::Error>> + Send>> {
        let this = self;
        Box::pin(stream! {
            let mut rx = this.spawn_task();

            while let Some(msg) = rx.recv().await {
                tracing::debug!("MESSAGE: {:?}", &msg);
                yield msg;
            }
        })
    }
}

impl<T, E> Drop for Subscription<T, E>
where
    T: for<'de> Deserialize<'de>,
    E: for<'de> Deserialize<'de>,
{
    fn drop(&mut self) {
        tracing::trace!("Dropping WebSocket subscription (stopping)...");
        self.tx
            .send(ClientMessage::Stop {
                id: self.id.clone(),
            })
            .unwrap_or(());
    }
}

impl Drop for GraphQLConnection {
    fn drop(&mut self) {
        tracing::trace!("Dropping WebSocket connection (terminating)...");
        self.tx
            .send(ClientMessage::ConnectionTerminate)
            .unwrap_or(());
    }
}

pub(crate) mod raw {
    use std::convert::TryFrom;

    use futures_util::stream::{SplitSink, SplitStream, Stream};
    use futures_util::{pin_mut, SinkExt, StreamExt};
    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncRead, AsyncWrite};
    use tokio_tungstenite::{
        tungstenite::protocol, tungstenite::Message, MaybeTlsStream, WebSocketStream,
    };

    use crate::{RequestBody, Response};

    #[derive(Debug, Clone, Serialize)]
    #[serde(tag = "type")]
    pub enum ClientMessage {
        #[serde(rename = "connection_init")]
        ConnectionInit {
            #[serde(skip_serializing_if = "Option::is_none")]
            payload: Option<serde_json::Value>,
        },

        #[serde(rename = "start")]
        Start { id: String, payload: RequestBody },

        #[serde(rename = "stop")]
        Stop { id: String },

        #[serde(rename = "connection_terminate")]
        ConnectionTerminate,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(tag = "type")]
    pub enum ServerMessage {
        #[serde(rename = "error")]
        ConnectionError { payload: serde_json::Value },

        #[serde(rename = "connection_ack")]
        ConnectionAck,

        #[serde(rename = "data")]
        Data {
            id: String,
            payload: Response<serde_json::Value>,
        },

        #[serde(rename = "error")]
        Error {
            id: String,
            payload: serde_json::Value,
        },

        #[serde(rename = "complete")]
        Complete { id: String },

        #[serde(rename = "ka")]
        ConnectionKeepAlive,
    }

    impl ServerMessage {
        pub fn id(&self) -> Option<u64> {
            let id_str = match self {
                ServerMessage::Data { id, .. } => id,
                ServerMessage::Error { id, .. } => id,
                ServerMessage::Complete { id } => id,
                _ => return None,
            };

            u64::from_str_radix(id_str, 16).ok()
        }
    }

    impl From<ClientMessage> for protocol::Message {
        fn from(message: ClientMessage) -> Self {
            Message::Text(serde_json::to_string(&message).unwrap())
        }
    }

    #[derive(Debug)]
    pub enum MessageError {
        Decoding(serde_json::Error),
        InvalidMessage(protocol::Message),
        WebSocket(tokio_tungstenite::tungstenite::Error),
    }

    impl TryFrom<protocol::Message> for ServerMessage {
        type Error = MessageError;

        fn try_from(value: protocol::Message) -> Result<Self, MessageError> {
            match value {
                Message::Text(value) => {
                    serde_json::from_str(&value).map_err(|e| MessageError::Decoding(e))
                }
                _ => Err(MessageError::InvalidMessage(value)),
            }
        }
    }

    pub struct GraphQLSender<S>
    where
        S: AsyncWrite + Unpin,
    {
        pub(crate) sink: SplitSink<WebSocketStream<MaybeTlsStream<S>>, Message>,
    }

    impl<S> GraphQLSender<S>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        pub async fn send(
            &mut self,
            message: ClientMessage,
        ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
            let sink = &mut self.sink;
            pin_mut!(sink);
            SinkExt::send(&mut sink, message.into()).await
        }
    }

    pub struct GraphQLReceiver<S>
    where
        S: AsyncRead + Unpin + Send,
    {
        pub(crate) stream: SplitStream<WebSocketStream<MaybeTlsStream<S>>>,
    }

    impl<S> GraphQLReceiver<S>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        pub fn stream(self) -> impl Stream<Item = Result<ServerMessage, MessageError>> + Send {
            StreamExt::map(self.stream, |x| match x {
                Ok(msg) => ServerMessage::try_from(msg),
                Err(e) => Err(MessageError::WebSocket(e)),
            })
        }
    }
}
