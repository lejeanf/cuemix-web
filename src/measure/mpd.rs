//! Minimal MPD client: the plain TCP line protocol, just what the room test
//! needs (`status`, `addid`, `playid`, `seekid`, `stop`, ...).
//!
//! Replies end with `OK` or `ACK [code@n] {command} message`. Every read and
//! the connect have a timeout, a line is capped, and the password is never
//! logged or echoed in an error.

use std::net::IpAddr;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Connect and per-line read/write timeout.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest line accepted from the server.
const MAX_LINE: u64 = 16 * 1024;
/// Most lines in one reply (`status` has about 25).
const MAX_LINES: usize = 512;

#[derive(Debug, Error)]
pub enum MpdError {
    #[error("MPD: {0}")]
    Io(#[from] std::io::Error),
    #[error("MPD did not answer in time")]
    Timeout,
    #[error("MPD refused: {0}")]
    Ack(String),
    #[error("not an MPD server or bad reply: {0}")]
    Protocol(String),
}

/// `play` / `pause` / `stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    Play,
    Pause,
    #[default]
    Stop,
}

/// What the room test reads from `status`. The four flags are kept as text so
/// they go back exactly as they were (`single` may be `oneshot`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    pub state: PlayState,
    pub song: Option<u32>,
    pub songid: Option<u32>,
    pub elapsed: Option<f64>,
    pub single: String,
    pub repeat: String,
    pub random: String,
    pub consume: String,
}

impl Status {
    /// Parse the `key: value` lines of a `status` reply.
    pub fn parse(lines: &[String]) -> Self {
        let mut s = Status::default();
        for line in lines {
            let Some((key, value)) = line.split_once(": ") else {
                continue;
            };
            match key {
                "state" => {
                    s.state = match value {
                        "play" => PlayState::Play,
                        "pause" => PlayState::Pause,
                        _ => PlayState::Stop,
                    }
                }
                "song" => s.song = value.parse().ok(),
                "songid" => s.songid = value.parse().ok(),
                "elapsed" => s.elapsed = value.parse().ok().filter(|e: &f64| e.is_finite()),
                "single" => s.single = value.to_owned(),
                "repeat" => s.repeat = value.to_owned(),
                "random" => s.random = value.to_owned(),
                "consume" => s.consume = value.to_owned(),
                _ => {}
            }
        }
        s
    }
}

/// Quote one argument: `"..."` with `\` and `"` escaped. Line breaks cannot be
/// carried by the protocol and are refused.
pub fn quote(arg: &str) -> Result<String, MpdError> {
    if arg.contains(['\n', '\r', '\0']) {
        return Err(MpdError::Protocol("argument contains a line break".into()));
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

/// An open, greeted (and authenticated) MPD connection.
#[derive(Debug)]
pub struct Mpd {
    stream: BufReader<TcpStream>,
    local_ip: IpAddr,
}

impl Mpd {
    /// Connect to `addr` (`host:port`), check the greeting, send the password
    /// when there is one.
    pub async fn connect(addr: &str, password: &str) -> Result<Self, MpdError> {
        let tcp = timeout(IO_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| MpdError::Timeout)??;
        let local_ip = tcp.local_addr()?.ip();
        let mut mpd = Self {
            stream: BufReader::new(tcp),
            local_ip,
        };
        let greeting = mpd.read_line().await?;
        if !greeting.starts_with("OK MPD ") {
            return Err(MpdError::Protocol("unexpected greeting".into()));
        }
        if !password.is_empty() {
            mpd.run(&format!("password {}", quote(password)?)).await?;
        }
        Ok(mpd)
    }

    /// Our address on this connection: the one MPD can reach us at.
    pub fn local_ip(&self) -> IpAddr {
        self.local_ip
    }

    async fn read_line(&mut self) -> Result<String, MpdError> {
        let mut line = String::new();
        let mut limited = (&mut self.stream).take(MAX_LINE);
        let n = timeout(IO_TIMEOUT, limited.read_line(&mut line))
            .await
            .map_err(|_| MpdError::Timeout)??;
        if n == 0 {
            return Err(MpdError::Protocol("connection closed".into()));
        }
        if !line.ends_with('\n') {
            return Err(MpdError::Protocol("line too long".into()));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    /// Send one command line, return the reply lines before `OK`.
    pub async fn run(&mut self, command: &str) -> Result<Vec<String>, MpdError> {
        let line = format!("{command}\n");
        timeout(IO_TIMEOUT, self.stream.write_all(line.as_bytes()))
            .await
            .map_err(|_| MpdError::Timeout)??;
        let mut lines = Vec::new();
        loop {
            let l = self.read_line().await?;
            if l == "OK" {
                return Ok(lines);
            }
            if let Some(ack) = l.strip_prefix("ACK ") {
                // `[50@0] {playid} No such song`: keep the human part.
                let msg = ack.split_once("} ").map_or(ack, |(_, m)| m);
                return Err(MpdError::Ack(msg.to_owned()));
            }
            if lines.len() >= MAX_LINES {
                return Err(MpdError::Protocol("reply too long".into()));
            }
            lines.push(l);
        }
    }

    pub async fn status(&mut self) -> Result<Status, MpdError> {
        Ok(Status::parse(&self.run("status").await?))
    }

    /// Queue `uri`, return its song id.
    pub async fn addid(&mut self, uri: &str) -> Result<u32, MpdError> {
        let lines = self.run(&format!("addid {}", quote(uri)?)).await?;
        lines
            .iter()
            .find_map(|l| l.strip_prefix("Id: "))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| MpdError::Protocol("addid returned no Id".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn status_is_parsed() {
        let s = Status::parse(&lines(
            "volume: 80\nrepeat: 1\nrandom: 0\nsingle: oneshot\nconsume: 0\nstate: pause\n\
             song: 3\nsongid: 17\nelapsed: 42.250\nduration: 200.0\nbitrate: 320",
        ));
        assert_eq!(s.state, PlayState::Pause);
        assert_eq!((s.song, s.songid), (Some(3), Some(17)));
        assert_eq!(s.elapsed, Some(42.25));
        assert_eq!(
            (&*s.single, &*s.repeat, &*s.random, &*s.consume),
            ("oneshot", "1", "0", "0")
        );
        let stopped = Status::parse(&lines("state: stop\nrepeat: 0"));
        assert_eq!(stopped.state, PlayState::Stop);
        assert_eq!(stopped.songid, None);
        assert_eq!(Status::parse(&lines("garbage\nsongid: x")).songid, None);
    }

    #[test]
    fn arguments_are_quoted() {
        assert_eq!(quote("plain").ok().as_deref(), Some("\"plain\""));
        assert_eq!(
            quote("a\"b\\c").ok().as_deref(),
            Some("\"a\\\"b\\\\c\""),
            "quote and backslash are escaped"
        );
        assert!(quote("two\nlines").is_err());
    }
}
