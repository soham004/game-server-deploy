//! A small RCON client, for servers older than the management protocol.
//!
//! Each packet is `length | id | type | text \0 \0`, in little-endian. A reply over 4096 bytes comes in
//! several packets, with nothing to say which is the last, so each command is followed by a second packet
//! the server answers only after it has finished with the first: its answer marks the end.

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const TYPE_RESPONSE: i32 = 0;
const TYPE_COMMAND: i32 = 2;
const TYPE_LOGIN: i32 = 3;
/// Two ids, one for what was asked and one for the marker after it.
const LOGIN_ID: i32 = 1;
const COMMAND_ID: i32 = 2;
const MARKER_ID: i32 = 3;
/// The protocol's largest packet; anything bigger isn't RCON.
const MAX_PACKET_BYTES: usize = 8192;
/// What a single reply may add up to.
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
/// How long one command may take to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum RconError {
    #[error("could not connect to RCON")]
    Connect(#[source] std::io::Error),
    #[error("RCON didn't answer in time")]
    Timeout,
    #[error("RCON refused the password")]
    Refused,
    #[error("the connection to RCON failed")]
    Io(#[from] std::io::Error),
    #[error("that isn't an RCON answer: {0}")]
    Invalid(&'static str),
}

/// A logged-in connection.
#[derive(Debug)]
pub struct Rcon {
    stream: TcpStream,
}

struct Packet {
    id: i32,
    kind: i32,
    text: Vec<u8>,
}

impl Rcon {
    /// Connects over loopback and logs in.
    pub async fn connect(port: u16, password: &str, wait: Duration) -> Result<Self, RconError> {
        let stream = timeout(wait, TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| RconError::Timeout)?
            .map_err(RconError::Connect)?;
        let mut client = Self { stream };
        client.send(LOGIN_ID, TYPE_LOGIN, password).await?;
        // A server may send an empty response first; the login's own answer has the login's type.
        loop {
            let packet = client.read_timed().await?;
            if packet.id == -1 {
                return Err(RconError::Refused);
            }
            if packet.kind == TYPE_COMMAND && packet.id == LOGIN_ID {
                return Ok(client);
            }
        }
    }

    /// Runs a command and returns all of its reply.
    pub async fn command(&mut self, command: &str) -> Result<String, RconError> {
        self.send(COMMAND_ID, TYPE_COMMAND, command).await?;
        // A response-type packet isn't a known request, so the server answers it, after the command.
        self.send(MARKER_ID, TYPE_RESPONSE, "").await?;
        let mut reply = Vec::new();
        loop {
            let packet = self.read_timed().await?;
            match packet.id {
                MARKER_ID => return Ok(String::from_utf8_lossy(&reply).into_owned()),
                COMMAND_ID => {
                    if reply.len() + packet.text.len() > MAX_REPLY_BYTES {
                        return Err(RconError::Invalid("the reply is too long"));
                    }
                    reply.extend_from_slice(&packet.text);
                }
                _ => return Err(RconError::Invalid("an answer to something not asked")),
            }
        }
    }

    async fn send(&mut self, id: i32, kind: i32, text: &str) -> Result<(), RconError> {
        let packet = encode(id, kind, text)?;
        self.stream.write_all(&packet).await?;
        Ok(())
    }

    async fn read_timed(&mut self) -> Result<Packet, RconError> {
        timeout(REPLY_TIMEOUT, self.read_packet())
            .await
            .map_err(|_| RconError::Timeout)?
    }

    async fn read_packet(&mut self) -> Result<Packet, RconError> {
        let length = usize::try_from(self.stream.read_i32_le().await?)
            .map_err(|_| RconError::Invalid("a negative length"))?;
        if !(10..=MAX_PACKET_BYTES).contains(&length) {
            return Err(RconError::Invalid("a length out of range"));
        }
        let mut body = vec![0; length];
        self.stream.read_exact(&mut body).await?;
        decode(&body)
    }
}

fn encode(id: i32, kind: i32, text: &str) -> Result<Vec<u8>, RconError> {
    let length = text.len() + 10;
    if length > MAX_PACKET_BYTES {
        return Err(RconError::Invalid("the command is too long"));
    }
    let mut packet = Vec::with_capacity(length + 4);
    let length = i32::try_from(length).map_err(|_| RconError::Invalid("the command is too long"))?;
    packet.extend_from_slice(&length.to_le_bytes());
    packet.extend_from_slice(&id.to_le_bytes());
    packet.extend_from_slice(&kind.to_le_bytes());
    packet.extend_from_slice(text.as_bytes());
    packet.extend_from_slice(&[0, 0]);
    Ok(packet)
}

fn decode(body: &[u8]) -> Result<Packet, RconError> {
    let number = |range: std::ops::Range<usize>| {
        body.get(range)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(i32::from_le_bytes)
            .ok_or(RconError::Invalid("a short packet"))
    };
    let (id, kind) = (number(0..4)?, number(4..8)?);
    let text = body
        .get(8..body.len().saturating_sub(2))
        .ok_or(RconError::Invalid("a short packet"))?;
    Ok(Packet {
        id,
        kind,
        text: text.to_vec(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    const PASSWORD: &str = "the password";

    async fn read_request(stream: &mut TcpStream) -> Option<Packet> {
        let length = usize::try_from(stream.read_i32_le().await.ok()?).ok()?;
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.ok()?;
        decode(&body).ok()
    }

    /// A server that behaves as Minecraft does: a long reply comes in pieces of 4096 bytes, and it
    /// answers an unknown packet with an empty response of the same id.
    async fn fake_server(reply: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            while let Some(request) = read_request(&mut stream).await {
                let mut answer = Vec::new();
                if request.kind == TYPE_LOGIN {
                    let good = request.text == PASSWORD.as_bytes();
                    let id = if good { request.id } else { -1 };
                    answer.extend(encode(id, TYPE_COMMAND, "").unwrap());
                } else if request.kind == TYPE_COMMAND {
                    for piece in reply.chunks(4096) {
                        let text = std::str::from_utf8(piece).unwrap();
                        answer.extend(encode(request.id, TYPE_RESPONSE, text).unwrap());
                    }
                } else {
                    answer.extend(encode(request.id, TYPE_RESPONSE, "").unwrap());
                }
                stream.write_all(&answer).await.unwrap();
            }
        });
        port
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn a_command_gets_its_reply() {
        let port = fake_server(b"There are 0 of a max of 20 players online".to_vec()).await;
        let mut rcon = Rcon::connect(port, PASSWORD, WAIT).await.unwrap();
        let reply = rcon.command("list").await.unwrap();
        assert!(reply.starts_with("There are 0"), "{reply}");
        assert!(!reply.contains('\0'));
    }

    #[tokio::test]
    async fn a_long_reply_comes_back_whole_and_an_empty_one_is_fine() {
        let long = "x".repeat(4096 * 3 + 7);
        let port = fake_server(long.clone().into_bytes()).await;
        let mut rcon = Rcon::connect(port, PASSWORD, WAIT).await.unwrap();
        assert_eq!(rcon.command("help").await.unwrap(), long);
        assert_eq!(
            rcon.command("help").await.unwrap(),
            long,
            "and the next command too"
        );

        let port = fake_server(Vec::new()).await;
        let mut rcon = Rcon::connect(port, PASSWORD, WAIT).await.unwrap();
        assert_eq!(rcon.command("save-all").await.unwrap(), "");
    }

    #[tokio::test]
    async fn a_wrong_password_is_refused() {
        let port = fake_server(Vec::new()).await;
        let refused = Rcon::connect(port, "wrong", WAIT).await;
        assert!(matches!(refused, Err(RconError::Refused)), "{refused:?}");
    }

    #[tokio::test]
    async fn nobody_listening_is_an_error_to_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let refused = Rcon::connect(port, PASSWORD, WAIT).await;
        assert!(matches!(refused, Err(RconError::Connect(_))), "{refused:?}");
    }

    #[tokio::test]
    async fn garbage_is_refused_without_reading_it_all() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = stream.write_i32_le(i32::MAX).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let result = Rcon::connect(port, PASSWORD, WAIT).await;
        assert!(matches!(result, Err(RconError::Invalid(_))), "{result:?}");
    }

    #[test]
    fn a_command_too_long_for_the_protocol_is_refused() {
        assert!(encode(COMMAND_ID, TYPE_COMMAND, &"x".repeat(MAX_PACKET_BYTES)).is_err());
    }
}
