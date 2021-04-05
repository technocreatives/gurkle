use std::{marker::PhantomData, pin::Pin};

use async_stream::stream;
use futures_util::{stream::Stream, StreamExt};
use raw::{ClientMessage, GraphQLReceiver, GraphQLSender, ServerMessage};
use reqwest::Url;
use serde::{de::DeserializeOwned, Deserialize};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::{connect_async, tungstenite};

pub use tungstenite::handshake::client::Request;
pub use tungstenite::Error;

use crate::RequestBody;

pub struct GraphQLWebSocket {
    tx: broadcast::Sender<ClientMessage>,
    server_tx: broadcast::Sender<ServerMessage>,
    #[allow(dead_code)] // Need this to avoid a hangup
    server_rx: broadcast::Receiver<ServerMessage>,
    id_count: u64,
}

impl GraphQLWebSocket {
    pub async fn connect(url: Url) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let req = Request::builder().uri(url.to_string()).body(()).unwrap();
        Self::connect_request(req).await
    }

    pub async fn connect_request(request: Request) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let (stream, _) = match connect_async(request).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let (sink, stream) = StreamExt::split(stream);

        let (tx_in, rx_in) = broadcast::channel(16);

        let tx_in0 = tx_in.clone();
        tokio::spawn(async move {
            let rx = GraphQLReceiver { stream };
            let mut stream = rx.stream();
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(ServerMessage::ConnectionKeepAlive) => {}
                    Ok(v) => {
                        let _ = tx_in0.send(v);
                    }
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        });

        let (tx_out, mut rx_out) = broadcast::channel(16);
        tokio::spawn(async move {
            let mut tx = GraphQLSender { sink };

            tx.send(ClientMessage::ConnectionInit { payload: None })
                .await
                .unwrap();

            while let Ok(msg) = rx_out.recv().await {
                match tx.send(msg).await {
                    Ok(()) => {}
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        });

        let socket = GraphQLWebSocket {
            tx: tx_out,
            server_tx: tx_in,
            server_rx: rx_in,
            id_count: 0,
        };

        Ok(socket)
    }

    pub fn subscribe<T>(&mut self, payload: RequestBody) -> Subscription<T>
    where
        T: for<'de> Deserialize<'de> + Unpin + Send + 'static,
    {
        self.id_count += 1;
        let id = format!("{:x}", self.id_count);

        let sub = Subscription::<T>::new(id, self.tx.clone(), self.server_tx.subscribe(), payload);

        sub
    }
}

pub struct Subscription<
    T: for<'de> Deserialize<'de> = serde_json::Value,
    E: for<'de> Deserialize<'de> = serde_json::Value,
> {
    id: String,
    tx: broadcast::Sender<ClientMessage>,
    rx: broadcast::Receiver<ServerMessage>,
    payload: RequestBody,
    ty_value: PhantomData<T>,
    ty_error: PhantomData<E>,
}

// pub enum SubscriptionError<T> {
//     InvalidData(Response<T>),
//     InternalError(serde_json::Value),
// }

impl<T, E> Subscription<T, E>
where
    T: DeserializeOwned + Unpin + Send + 'static,
    E: DeserializeOwned + Unpin + Send + 'static,
{
    pub fn new(
        id: String,
        tx: broadcast::Sender<ClientMessage>,
        rx: broadcast::Receiver<ServerMessage>,
        payload: RequestBody,
    ) -> Self {
        Self {
            id,
            tx,
            rx,
            payload,
            ty_value: PhantomData,
            ty_error: PhantomData,
        }
    }

    fn spawn_task(self) -> mpsc::Receiver<Result<serde_json::Value, crate::Error>> {
        let mut this = self;
        let id = this.id.clone();
        let payload = this.payload.clone();
        let (tx, rx) = mpsc::channel(16);

        tokio::spawn(async move {
            tracing::trace!("Sending start message");
            this.tx.send(ClientMessage::Start { id, payload }).unwrap();

            tracing::trace!("Sent!");

            while let Ok(msg) = this.rx.recv().await {
                tracing::trace!("{:?}", &msg);
                match msg {
                    ServerMessage::Data { id, payload } => {
                        if id == this.id {
                            // let raw_data = payload.data.unwrap_or(serde_json::Value::Null);
                            // let raw_errors = payload.errors.unwrap_or(serde_json::Value::Null);

                            // let data: Option<T> = serde_json::from_value(raw_data).unwrap_or(None);
                            // let errors: Option<E> =
                            //     serde_json::from_value(raw_errors).unwrap_or(None);

                            let _ = tx.send(payload.into()).await;
                        }
                    }
                    ServerMessage::Complete { id } => {
                        if id == this.id {
                            return;
                        }
                    }
                    ServerMessage::ConnectionError { payload } => {
                        let _ = tx.send(Err(crate::Error::Server(payload))).await;
                        return;
                    }
                    ServerMessage::Error { id, payload } => {
                        if id == this.id {
                            let _ = tx.send(Err(crate::Error::Server(payload))).await;
                        }
                    }
                    ServerMessage::ConnectionAck => {}
                    ServerMessage::ConnectionKeepAlive => {}
                }
            }
        });

        rx
    }

    pub fn stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = Result<serde_json::Value, crate::Error>> + Send>> {
        let this = self;
        Box::pin(stream! {
            let mut rx = this.spawn_task();

            while let Some(msg) = rx.recv().await {
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
            .unwrap_or(0);
    }
}

impl Drop for GraphQLWebSocket {
    fn drop(&mut self) {
        tracing::trace!("Dropping WebSocket connection (terminating)...");
        self.tx
            .send(ClientMessage::ConnectionTerminate)
            .unwrap_or(0);
    }
}

pub mod raw {
    use std::convert::TryFrom;

    use futures_util::stream::{SplitSink, SplitStream, Stream};
    use futures_util::{pin_mut, SinkExt, StreamExt};
    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncRead, AsyncWrite};
    use tokio_tungstenite::{
        tungstenite::protocol, tungstenite::Message, MaybeTlsStream, WebSocketStream,
    };

    use crate::{RequestBody, Response};

    // #[derive(Debug, Clone, Serialize, Deserialize)]
    // #[serde(rename = "camelCase")]
    // pub struct RequestBody {
    //     pub query: String,
    //     #[serde(skip_serializing_if = "Option::is_none")]
    //     pub variables: Option<serde_json::Value>,
    //     #[serde(skip_serializing_if = "Option::is_none")]
    //     pub operation_name: Option<String>,
    // }

    // #[derive(Debug, Clone, Serialize, Deserialize)]
    // #[serde(rename = "camelCase")]
    // pub struct Payload<T = serde_json::Value, E = serde_json::Value> {
    //     #[serde(skip_serializing_if = "Option::is_none")]
    //     pub data: Option<T>,
    //     #[serde(skip_serializing_if = "Option::is_none")]
    //     pub errors: Option<E>,
    // }

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

    // impl ServerMessage {
    //     pub fn id(&self) -> Option<&str> {
    //         match self {
    //             ServerMessage::Data { id, .. } => Some(&id),
    //             ServerMessage::Error { id, .. } => Some(&id),
    //             ServerMessage::Complete { id } => Some(&id),
    //             _ => None,
    //         }
    //     }
    // }

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
                tracing::trace!("{:?}", &x);

                match x {
                    Ok(msg) => ServerMessage::try_from(msg),
                    Err(e) => Err(MessageError::WebSocket(e)),
                }
            })
        }
    }
}
