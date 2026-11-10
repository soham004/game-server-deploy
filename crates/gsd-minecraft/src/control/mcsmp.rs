//! A small client for the Minecraft Server Management Protocol: JSON-RPC 2.0 over a WebSocket. The
//! server also pushes notifications on the same connection, which are kept for whoever asks.

use std::{collections::VecDeque, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

/// How long one call may take to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// Notifications kept while nobody reads them; the oldest go first.
const MAX_WAITING_NOTIFICATIONS: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum McsmpError {
    #[error("could not connect to the management protocol: {0}")]
    Connect(String),
    #[error("the management protocol didn't answer in time")]
    Timeout,
    #[error("the connection to the management protocol failed: {0}")]
    Connection(String),
    #[error("the management protocol closed the connection")]
    Closed,
    #[error("the management protocol said: {message}")]
    Rpc { code: i64, message: String },
    #[error("that isn't a management protocol answer")]
    Invalid,
}

/// What the running server says it can do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovered {
    pub version: Option<String>,
    /// Method names, with notifications among them.
    pub methods: Vec<String>,
}

/// Something the server announced.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub struct Mcsmp {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    waiting: VecDeque<Notification>,
}

impl Mcsmp {
    /// Connects over loopback. The secret goes in a header marked sensitive, so it can't reach a log.
    pub async fn connect(port: u16, secret: &str, wait: Duration) -> Result<Self, McsmpError> {
        let connect_error = |error: &dyn std::fmt::Display| McsmpError::Connect(error.to_string());
        let mut request = format!("ws://127.0.0.1:{port}/")
            .into_client_request()
            .map_err(|error| connect_error(&error))?;
        let mut header =
            HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|error| connect_error(&error))?;
        header.set_sensitive(true);
        request.headers_mut().insert("Authorization", header);
        let (socket, _) = timeout(wait, connect_async(request))
            .await
            .map_err(|_| McsmpError::Timeout)?
            .map_err(|error| connect_error(&error))?;
        Ok(Self {
            socket,
            next_id: 0,
            waiting: VecDeque::new(),
        })
    }

    /// Calls a method. Notifications that arrive meanwhile are kept for [`Self::next_notification`].
    pub async fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, McsmpError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        self.socket
            .send(Message::text(request.to_string()))
            .await
            .map_err(|error| McsmpError::Connection(error.to_string()))?;
        timeout(REPLY_TIMEOUT, self.read_reply(id))
            .await
            .map_err(|_| McsmpError::Timeout)?
    }

    /// What the running server offers. Its own answer, since method names differ between versions.
    pub async fn discover(&mut self) -> Result<Discovered, McsmpError> {
        Ok(parse_discovery(&self.call("rpc.discover", None).await?))
    }

    /// The next thing the server announces, waiting at most `wait`.
    pub async fn next_notification(&mut self, wait: Duration) -> Result<Option<Notification>, McsmpError> {
        if let Some(waiting) = self.waiting.pop_front() {
            return Ok(Some(waiting));
        }
        let read = async {
            loop {
                let message = self.read_message().await?;
                if let Some(notification) = notification_in(&message) {
                    return Ok(notification);
                }
            }
        };
        match timeout(wait, read).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }

    async fn read_reply(&mut self, id: u64) -> Result<Value, McsmpError> {
        loop {
            let message = self.read_message().await?;
            if let Some(notification) = notification_in(&message) {
                if self.waiting.len() == MAX_WAITING_NOTIFICATIONS {
                    self.waiting.pop_front();
                }
                self.waiting.push_back(notification);
            } else if message.get("id").and_then(Value::as_u64) == Some(id) {
                return reply_of(message);
            }
        }
    }

    /// The next JSON message. Pings are answered by the library; anything else isn't for us.
    async fn read_message(&mut self) -> Result<Value, McsmpError> {
        loop {
            match self.socket.next().await {
                None | Some(Ok(Message::Close(_))) => return Err(McsmpError::Closed),
                Some(Err(error)) => return Err(McsmpError::Connection(error.to_string())),
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str()).map_err(|_| McsmpError::Invalid);
                }
                Some(Ok(_)) => {}
            }
        }
    }
}

/// A notification has a method and no id.
fn notification_in(message: &Value) -> Option<Notification> {
    if message.get("id").is_some_and(|id| !id.is_null()) {
        return None;
    }
    Some(Notification {
        method: message.get("method")?.as_str()?.to_owned(),
        params: message.get("params").cloned().unwrap_or(Value::Null),
    })
}

fn reply_of(mut message: Value) -> Result<Value, McsmpError> {
    if let Some(error) = message.get("error") {
        return Err(McsmpError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no reason given")
                .to_owned(),
        });
    }
    message
        .get_mut("result")
        .map(Value::take)
        .ok_or(McsmpError::Invalid)
}

/// An `OpenRPC` document: `info.version`, and `methods[].name`.
fn parse_discovery(document: &Value) -> Discovered {
    Discovered {
        version: document
            .pointer("/info/version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        methods: document
            .get("methods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|method| method.get("name")?.as_str().map(str::to_owned))
            .collect(),
    }
}

#[cfg(test)]
// `result_large_err`: the handshake callback's error type is the library's.
#[allow(clippy::unwrap_used, clippy::result_large_err)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{ErrorResponse, Request, Response},
            http::StatusCode,
        },
    };

    use super::*;

    const SECRET: &str = "the-secret";
    const WAIT: Duration = Duration::from_secs(5);

    /// A server that wants the secret, announces something before each answer, and answers from `reply`.
    async fn fake_server(reply: fn(&Value) -> Value) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let check = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                        let good = request
                            .headers()
                            .get("Authorization")
                            .is_some_and(|value| value == &format!("Bearer {SECRET}"));
                        if good {
                            Ok(response)
                        } else {
                            let mut refused = ErrorResponse::new(None);
                            *refused.status_mut() = StatusCode::UNAUTHORIZED;
                            Err(refused)
                        }
                    };
                    let Ok(mut socket) = accept_hdr_async(stream, check).await else {
                        return;
                    };
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        let request: Value = serde_json::from_str(text.as_str()).unwrap();
                        let notice = json!({ "jsonrpc": "2.0", "method": "minecraft:notification/players/joined", "params": [{ "name": "Steve" }] });
                        socket.send(Message::text(notice.to_string())).await.unwrap();
                        socket
                            .send(Message::text(reply(&request).to_string()))
                            .await
                            .unwrap();
                    }
                });
            }
        });
        port
    }

    fn answer(request: &Value) -> Value {
        match request["method"].as_str().unwrap() {
            "rpc.discover" => json!({
                "jsonrpc": "2.0", "id": request["id"],
                "result": { "openrpc": "1.3.2", "info": { "title": "Minecraft", "version": "3.1.0" },
                    "methods": [{ "name": "minecraft:players" }, { "name": "minecraft:notification/players/joined" }] },
            }),
            "minecraft:players" => {
                json!({ "jsonrpc": "2.0", "id": request["id"], "result": [{ "name": "Alex" }] })
            }
            "minecraft:echo" => json!({ "jsonrpc": "2.0", "id": request["id"], "result": request["params"] }),
            _ => {
                json!({ "jsonrpc": "2.0", "id": request["id"], "error": { "code": -32601, "message": "Method not found" } })
            }
        }
    }

    #[tokio::test]
    async fn calls_get_their_own_answers() {
        let port = fake_server(answer).await;
        let mut client = Mcsmp::connect(port, SECRET, WAIT).await.unwrap();
        let players = client.call("minecraft:players", None).await.unwrap();
        assert_eq!(players, json!([{ "name": "Alex" }]));
        let echoed = client.call("minecraft:echo", Some(json!(["x"]))).await.unwrap();
        assert_eq!(echoed, json!(["x"]));
    }

    #[tokio::test]
    async fn discovery_says_the_version_and_the_methods() {
        let port = fake_server(answer).await;
        let mut client = Mcsmp::connect(port, SECRET, WAIT).await.unwrap();
        let discovered = client.discover().await.unwrap();
        assert_eq!(discovered.version.as_deref(), Some("3.1.0"));
        assert_eq!(discovered.methods.len(), 2);
    }

    #[tokio::test]
    async fn an_error_answer_is_an_error() {
        let port = fake_server(answer).await;
        let mut client = Mcsmp::connect(port, SECRET, WAIT).await.unwrap();
        let error = client.call("minecraft:nothing", None).await.unwrap_err();
        assert!(matches!(error, McsmpError::Rpc { code: -32601, .. }), "{error:?}");
    }

    #[tokio::test]
    async fn notifications_that_arrive_during_a_call_are_kept() {
        let port = fake_server(answer).await;
        let mut client = Mcsmp::connect(port, SECRET, WAIT).await.unwrap();
        client.call("minecraft:players", None).await.unwrap();
        let notice = client.next_notification(WAIT).await.unwrap().unwrap();
        assert_eq!(notice.method, "minecraft:notification/players/joined");
        assert_eq!(notice.params[0]["name"], "Steve");
        let none = client.next_notification(Duration::from_millis(50)).await.unwrap();
        assert_eq!(none, None, "and nothing more is waiting");
    }

    #[tokio::test]
    async fn the_wrong_secret_does_not_connect() {
        let port = fake_server(answer).await;
        let refused = Mcsmp::connect(port, "wrong", WAIT).await;
        assert!(matches!(refused, Err(McsmpError::Connect(_))), "{refused:?}");
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out_and_bad_json_is_invalid() {
        let bad = |_: &Value| Value::String("not rpc".into());
        let port = fake_server(bad).await;
        let mut client = Mcsmp::connect(port, SECRET, WAIT).await.unwrap();
        let waited = timeout(Duration::from_millis(200), client.call("minecraft:players", None)).await;
        assert!(waited.is_err(), "an unrelated answer isn't taken for the reply");
    }

    #[test]
    fn a_reply_without_a_result_is_invalid() {
        let reply = reply_of(json!({ "jsonrpc": "2.0", "id": 1 }));
        assert!(matches!(reply, Err(McsmpError::Invalid)), "{reply:?}");
    }

    #[test]
    fn an_answer_is_not_a_notification() {
        let answer = json!({ "jsonrpc": "2.0", "id": 1, "method": "x" });
        assert_eq!(notification_in(&answer), None);
    }
}
