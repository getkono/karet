//! Skeleton tests: a throwaway protocol with its own framing proves the broker
//! machinery carries no LSP (or `Content-Length`) assumptions.

use std::io;
use std::net::SocketAddr;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::tcp::OwnedWriteHalf;

use super::*;

/// Newline-delimited JSON — deliberately not the LSP framing.
#[derive(Clone, Copy, Debug)]
struct LineFraming;

impl Framing for LineFraming {
    async fn read_message<R>(reader: &mut R) -> io::Result<Option<Value>>
    where
        R: AsyncBufRead + Unpin + Send,
    {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    async fn write_message<W>(writer: &mut W, message: &Value) -> io::Result<()>
    where
        W: AsyncWrite + Unpin + Send,
    {
        let mut bytes = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        writer.write_all(&bytes).await?;
        writer.flush().await
    }
}

#[derive(Clone, Copy, Debug)]
struct TestProtocol;

impl BrokerProtocol for TestProtocol {
    type State = ();
    type RequestTag = ();
    type Framing = LineFraming;

    const STATE_DIR: &'static str = "test-brokers";
    const PROTOCOL_VERSION: &'static str = "t1";
    const MODE_ENV: &'static str = "KARET_INTERNAL_TEST_BROKER";
    const SPEC_ENV: &'static str = "KARET_INTERNAL_TEST_BROKER_SPEC";
    const PRELUDE: &'static str = "KARET-TEST-BROKER ";
    const DISPLAY_NAME: &'static str = "test broker";

    async fn on_client_message(
        message: &mut Value,
        _link: &ClientLink<'_, Self>,
    ) -> ClientFlow<Self::RequestTag> {
        if message.get("method").and_then(Value::as_str) == Some("stop") {
            return ClientFlow::Stop;
        }
        if message.get("id").is_some() && message.get("method").is_some() {
            return ClientFlow::Proxy(());
        }
        ClientFlow::Forward
    }

    fn route_server_message(message: &Value) -> ServerRoute {
        if message.get("id").is_some() && message.get("method").is_some() {
            ServerRoute::SingleClient
        } else {
            ServerRoute::AllClients
        }
    }
}

type BoxError = Box<dyn std::error::Error>;
type Client = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

struct Harness {
    core: Arc<Core<TestProtocol>>,
    address: SocketAddr,
    token: String,
    upstream: mpsc::Receiver<Value>,
    server: tokio::io::DuplexStream,
}

impl Harness {
    async fn start(token: &str) -> Result<Self, BoxError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let (upstream_tx, upstream) = mpsc::channel(16);
        let core = Core::<TestProtocol>::new(upstream_tx);
        let (server, brokered) = tokio::io::duplex(64 * 1024);
        tokio::spawn(upstream_reader::<TestProtocol, _>(
            brokered,
            Arc::clone(&core),
        ));
        let accepting = Arc::clone(&core);
        let accept_token = token.to_owned();
        tokio::spawn(async move {
            accept_clients::<TestProtocol>(&listener, &accept_token, accepting).await
        });
        Ok(Self {
            core,
            address,
            token: token.to_owned(),
            upstream,
            server,
        })
    }

    async fn client(&self) -> Result<Client, BoxError> {
        self.raw_client(&format!("{}{}", TestProtocol::PRELUDE, self.token))
            .await
    }

    async fn raw_client(&self, prelude: &str) -> Result<Client, BoxError> {
        let stream = TcpStream::connect(self.address).await?;
        let (read, mut write) = stream.into_split();
        write.write_all(format!("{prelude}\n").as_bytes()).await?;
        Ok((BufReader::new(read), write))
    }

    async fn push_from_server(&mut self, message: &Value) -> Result<(), BoxError> {
        LineFraming::write_message(&mut self.server, message).await?;
        Ok(())
    }

    async fn next_upstream(&mut self) -> Result<Value, BoxError> {
        tokio::time::timeout(Duration::from_secs(5), self.upstream.recv())
            .await?
            .ok_or_else(|| BoxError::from("upstream channel closed"))
    }

    async fn wait_for_clients(&self, count: usize) -> Result<(), BoxError> {
        for _ in 0..500 {
            if self.core.clients.lock().await.len() == count {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Err(BoxError::from("clients never registered"))
    }
}

async fn send(writer: &mut OwnedWriteHalf, message: &Value) -> Result<(), BoxError> {
    LineFraming::write_message(writer, message).await?;
    Ok(())
}

async fn recv(reader: &mut BufReader<OwnedReadHalf>) -> Result<Value, BoxError> {
    tokio::time::timeout(Duration::from_secs(5), LineFraming::read_message(reader))
        .await??
        .ok_or_else(|| BoxError::from("client stream ended"))
}

#[tokio::test]
async fn a_wrong_prelude_closes_the_connection() -> Result<(), BoxError> {
    let harness = Harness::start("correct-token").await?;
    let (mut reader, _writer) = harness
        .raw_client(&format!("{}wrong-token", TestProtocol::PRELUDE))
        .await?;
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await??;
    assert_eq!(read, 0);
    Ok(())
}

#[tokio::test]
async fn a_request_is_renumbered_upstream_and_answered_with_its_original_id() -> Result<(), BoxError>
{
    let mut harness = Harness::start("token").await?;
    let (mut reader, mut writer) = harness.client().await?;
    send(
        &mut writer,
        &json!({"jsonrpc": "2.0", "id": "client-abc", "method": "ping", "params": null}),
    )
    .await?;

    let forwarded = harness.next_upstream().await?;
    assert_eq!(forwarded.get("id"), Some(&json!(1)));
    assert_eq!(forwarded.get("method"), Some(&json!("ping")));

    harness
        .push_from_server(&json!({"jsonrpc": "2.0", "id": 1, "result": "pong"}))
        .await?;
    let reply = recv(&mut reader).await?;
    assert_eq!(reply.get("id"), Some(&json!("client-abc")));
    assert_eq!(reply.get("result"), Some(&json!("pong")));
    Ok(())
}

#[tokio::test]
async fn a_server_notification_reaches_every_client() -> Result<(), BoxError> {
    let mut harness = Harness::start("token").await?;
    let (mut first, _first_writer) = harness.client().await?;
    let (mut second, _second_writer) = harness.client().await?;
    harness.wait_for_clients(2).await?;

    harness
        .push_from_server(&json!({"jsonrpc": "2.0", "method": "note", "params": {}}))
        .await?;
    assert_eq!(recv(&mut first).await?.get("method"), Some(&json!("note")));
    assert_eq!(recv(&mut second).await?.get("method"), Some(&json!("note")));
    Ok(())
}

#[tokio::test]
async fn a_server_request_is_routed_to_a_single_client() -> Result<(), BoxError> {
    let mut harness = Harness::start("token").await?;
    let (mut reader, _writer) = harness.client().await?;
    harness.wait_for_clients(1).await?;

    harness
        .push_from_server(&json!({"jsonrpc": "2.0", "id": 7, "method": "ask", "params": {}}))
        .await?;
    let received = recv(&mut reader).await?;
    assert_eq!(received.get("method"), Some(&json!("ask")));
    assert_eq!(received.get("id"), Some(&json!(7)));
    Ok(())
}

#[tokio::test]
async fn a_stop_flow_ends_the_client_session() -> Result<(), BoxError> {
    let harness = Harness::start("token").await?;
    let (mut reader, mut writer) = harness.client().await?;
    harness.wait_for_clients(1).await?;

    send(&mut writer, &json!({"jsonrpc": "2.0", "method": "stop"})).await?;
    harness.wait_for_clients(0).await?;
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await??;
    assert_eq!(read, 0);
    Ok(())
}
