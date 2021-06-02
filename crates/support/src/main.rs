use futures_util::StreamExt;
use gurkle::WsClient;

include!("./counter.generated.rs");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let client = WsClient::new(
        &"ws://localhost:4000/".parse().unwrap(),
        None,
        vec!["graphql-ws".into()],
    )
    .await
    .unwrap();

    let mut sub = CounterRequest {}.subscribe(&client).await.unwrap();

    while let Some(msg) = sub.next().await {
        println!("{:?}", msg);
    }
}
