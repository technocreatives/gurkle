use std::{collections::HashMap, marker::PhantomData, pin::Pin, sync::Arc};

use async_stream::stream;
use futures_util::{stream::Stream, StreamExt};
use raw::{ClientMessage, GraphQLReceiver, GraphQLSender, ServerMessage};
use serde::{de::DeserializeOwned, Deserialize};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite};
use tracing::Instrument;

pub use tungstenite::handshake::client::Request;
pub use tungstenite::Error;

use crate::RequestBody;

pub(crate) struct GraphQLWebSocket {
    tx: mpsc::UnboundedSender<ClientMessage>,
    subscriptions: Arc<RwLock<HashMap<u64, mpsc::UnboundedSender<ServerMessage>>>>,
    // server_tx: mpsc::UnboundedSender<ServerMessage>,
    // #[allow(dead_code)] // Need this to avoid a hangup
    // server_rx: mpsc::UnboundedReceiver<ServerMessage>,
    id_count: u64,
}

async fn process_server_message(
    // tx: &mut mpsc::UnboundedSender<ServerMessage>,
    msg: ServerMessage,
    subscriptions: &RwLock<HashMap<u64, mpsc::UnboundedSender<ServerMessage>>>,
) {
    if let Some(id) = msg.id() {
        let guard = subscriptions.read().await;

        let requires_cleanup = if let Some(tx) = guard.get(&id) {
            let requires_cleanup = match &msg {
                ServerMessage::Error { .. } | ServerMessage::Complete { .. } => true,
                _ => false,
            };

            tx.send(msg).is_err() || requires_cleanup
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
            let _ = tx.send(msg.clone());
        }
    };
}

impl GraphQLWebSocket {
    pub async fn connect(request: Request) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let (stream, _) = match connect_async(request).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let subscriptions = Arc::new(RwLock::new(HashMap::new()));

        let (sink, stream) = StreamExt::split(stream);

        // let (tx_in, rx_in) = mpsc::unbounded_channel();

        // let tx_in0 = tx_in.clone();
        let subs0 = subscriptions.clone();
        let span = tracing::trace_span!("receiver");
        tokio::spawn(async move {
            // let mut tx = tx_in0;
            let rx = GraphQLReceiver { stream };
            let subscriptions = subs0;

            let mut stream = rx.stream();
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(ServerMessage::ConnectionKeepAlive) => {}
                    Ok(v) => process_server_message(v, &*subscriptions).await,
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        }.instrument(span));

        let (tx_out, mut rx_out) = mpsc::unbounded_channel();
        let span = tracing::trace_span!("sender");
        tokio::spawn(async move {
            let mut tx = GraphQLSender { sink };

            tx.send(ClientMessage::ConnectionInit { payload: None })
                .await
                .unwrap();

            while let Some(msg) = rx_out.recv().await {
                match tx.send(msg).await {
                    Ok(()) => {}
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        }.instrument(span));

        let socket = GraphQLWebSocket {
            tx: tx_out,
            subscriptions,
            // server_tx: tx_in,
            // server_rx: rx_in,
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
            lock.insert(self.id_count, tx);
        }

        tracing::trace!("Sending start message");
        {
            let id = id.clone();
            let payload = payload.clone();
            self.tx.send(ClientMessage::Start { id, payload }).unwrap();
        }
        tracing::trace!("Sent!");

        // TODO: check for errors here, so we can exit early.

        let sub = Subscription::<T>::new(id, self.tx.clone(), rx);
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

        tokio::spawn(async move {
            let mut this = this;

            while let Some(msg) = this.rx.recv().await {
                tracing::trace!("{:?}", &msg);
                match msg {
                    ServerMessage::Data { id, payload } => {
                        if id == this.id {
                            match tx.send(payload.into()) {
                                Ok(_) => {
                                    tracing::trace!("Send payload successful.");
                                },
                                Err(e) => tracing::error!("{:?}", e),
                            }
                        } else {
                            tracing::error!("Subscription is receiving invalid messages! Got: {}", id);
                        }
                    }
                    ServerMessage::Complete { id } => {
                        if id == this.id {
                            return;
                        }
                    }
                    ServerMessage::ConnectionError { payload } => {
                        match tx.send(Err(crate::Error::Server(payload))) {
                            Ok(_) => {},
                            Err(e) => tracing::error!("{:?}", e),
                        }
                        return;
                    }
                    ServerMessage::Error { id, payload } => {
                        if id == this.id {
                            match tx.send(Err(crate::Error::Server(payload))) {
                                Ok(_) => {},
                                Err(e) => tracing::error!("{:?}", e),
                            }
                        }
                    }
                    ServerMessage::ConnectionAck => {}
                    ServerMessage::ConnectionKeepAlive => {}
                }
            }
        }.instrument(span));

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
        self.tx.send(ClientMessage::Stop {
            id: self.id.clone(),
        }).unwrap_or(());
    }
}

impl Drop for GraphQLWebSocket {
    fn drop(&mut self) {
        tracing::trace!("Dropping WebSocket connection (terminating)...");
        self.tx.send(ClientMessage::ConnectionTerminate).unwrap_or(());
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
            StreamExt::map(self.stream, |x| {
                match x {
                    Ok(msg) => ServerMessage::try_from(msg),
                    Err(e) => Err(MessageError::WebSocket(e)),
                }
            })
        }
    }
}
