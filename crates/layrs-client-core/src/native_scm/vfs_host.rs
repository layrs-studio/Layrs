//! One foreground provider host per VFS Worktree. The executable that launches
//! it must retain this process; dropping a CLI request must not drop the host.
//! Control is loopback-only, bounded, and authenticated by a private random
//! capability. Neither Debug nor error messages print its token.
use super::{vfs_worktree::VfsWorktree, NativeClientError, Result};
use layrs_core::scm::{StoreId, WorktreeId};
use layrs_store_local::scm_v1::vfs_recovery::VfsRecovery;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    path::PathBuf,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 64 * 1024;
const PROTOCOL: &str = "layrs.vfs-host.v1";
const CONTROL_FRAME_TIMEOUT: Duration = Duration::from_secs(3);

/// Non-secret launch result. Persist or pass this ticket to control clients;
/// the authentication token remains in the owner-only Store recovery directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VfsHostTicket {
    pub store_root: PathBuf,
    pub store_identity: String,
    pub store_metadata_identity: String,
    pub store_id: StoreId,
    pub worktree_id: WorktreeId,
    pub endpoint_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Endpoint {
    protocol: String,
    address: SocketAddrV4,
    token: String,
    worktree_id: WorktreeId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum VfsHostCommand {
    // Empty structs enforce deny_unknown_fields for internally tagged commands;
    // Serde's unit-variant visitor otherwise ignores the remaining map fields.
    Status {},
    /// The owning application has stopped its writers until this reply.
    Capture {
        writers_quiesced: bool,
    },
    Export {
        parent: PathBuf,
        writers_quiesced: bool,
    },
    Stop {},
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: String,
    token: String,
    command: VfsHostCommand,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum VfsHostResponse {
    Ok { value: serde_json::Value },
    Error { code: String, message: String },
}

pub struct VfsHost {
    worktree: VfsWorktree,
    listener: TcpListener,
    endpoint: Endpoint,
    ticket: VfsHostTicket,
}

impl VfsHost {
    /// Binding publishes only local private control metadata. It does not
    /// detach, install a service, or alter any OS feature.
    pub fn bind(worktree: VfsWorktree) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let SocketAddr::V4(address) = listener.local_addr()? else {
            return Err(NativeClientError::InvalidState(
                "VFS host is not IPv4 loopback",
            ));
        };
        let endpoint = Endpoint {
            protocol: PROTOCOL.into(),
            address,
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            worktree_id: worktree.worktree_id().clone(),
        };
        let ticket = VfsHostTicket {
            store_root: worktree.store_root().into(),
            store_identity: worktree.store_identity().into(),
            store_metadata_identity: worktree.metadata_identity().into(),
            store_id: worktree.store_id().clone(),
            worktree_id: worktree.worktree_id().clone(),
            endpoint_id: uuid::Uuid::new_v4().to_string(),
        };
        let bytes = serde_json::to_vec(&endpoint).map_err(io::Error::other)?;
        worktree
            .journal()
            .publish_endpoint(&ticket.endpoint_id, &bytes)?;
        Ok(Self {
            worktree,
            listener,
            endpoint,
            ticket,
        })
    }

    pub fn ticket(&self) -> &VfsHostTicket {
        &self.ticket
    }

    pub fn checkout_root(&self) -> &std::path::Path {
        self.worktree.checkout_root()
    }

    /// Runs until an authenticated Stop, a listener failure, or process exit.
    /// All exits preserve the persistent checkout and its unaccepted writes.
    pub fn serve(mut self) -> Result<()> {
        loop {
            let (mut stream, peer) = self.listener.accept()?;
            if !peer.ip().is_loopback() {
                continue;
            }
            let deadline = Instant::now() + CONTROL_FRAME_TIMEOUT;
            let request =
                match read_authenticated_request(&mut stream, &self.endpoint.token, deadline) {
                    Ok(request) => request,
                    _ => {
                        let _ = write_frame(
                            &mut DeadlineStream {
                                stream: &mut stream,
                                deadline,
                            },
                            &VfsHostResponse::Error {
                                code: "invalid_vfs_control_request".into(),
                                message: "VFS control request rejected".into(),
                            },
                        );
                        continue;
                    }
                };
            let stop = matches!(request.command, VfsHostCommand::Stop {});
            let result = self.execute(request.command);
            let stopped = stop && result.is_ok();
            let response = match result {
                Ok(value) => VfsHostResponse::Ok { value },
                Err(error) => VfsHostResponse::Error {
                    code: error.code().into(),
                    message: error.to_string(),
                },
            };
            // A lost reply cannot undo acceptance. Its durable receipt remains
            // available after restart; never retry the mutation inside serve.
            let _ = write_frame(
                &mut DeadlineStream {
                    stream: &mut stream,
                    deadline: Instant::now() + CONTROL_FRAME_TIMEOUT,
                },
                &response,
            );
            if stopped {
                return Ok(());
            }
        }
    }

    fn execute(&mut self, command: VfsHostCommand) -> Result<serde_json::Value> {
        match command {
            VfsHostCommand::Status {} => json(self.worktree.status()?),
            VfsHostCommand::Capture {
                writers_quiesced: true,
            } => json(self.worktree.capture_quiesced()?),
            VfsHostCommand::Export {
                parent,
                writers_quiesced: true,
            } => json(self.worktree.export_quiesced(&parent)?),
            VfsHostCommand::Capture { .. } | VfsHostCommand::Export { .. } => Err(
                NativeClientError::InvalidState("stop Worktree writers before checkpointing"),
            ),
            VfsHostCommand::Stop {} => {
                self.worktree.disconnect()?;
                Ok(serde_json::json!({ "disconnected": true, "workRetained": true }))
            }
        }
    }
}

pub struct VfsHostClient {
    endpoint: Endpoint,
}
impl VfsHostClient {
    pub fn open(ticket: &VfsHostTicket) -> Result<Self> {
        let bytes = VfsRecovery::read_endpoint(
            &ticket.store_root,
            &ticket.store_identity,
            &ticket.store_metadata_identity,
            &ticket.store_id,
            &ticket.worktree_id,
            &ticket.endpoint_id,
        )?;
        let endpoint: Endpoint = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if endpoint.protocol != PROTOCOL
            || endpoint.address.ip() != &Ipv4Addr::LOCALHOST
            || endpoint.worktree_id != ticket.worktree_id
            || endpoint.token.len() != 64
            || !endpoint.token.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(NativeClientError::InvalidState(
                "invalid VFS control endpoint",
            ));
        }
        Ok(Self { endpoint })
    }
    pub fn request(&self, command: VfsHostCommand) -> Result<VfsHostResponse> {
        self.request_with_timeout(command, Duration::from_secs(300))
    }

    pub fn request_with_timeout(
        &self,
        command: VfsHostCommand,
        timeout: Duration,
    ) -> Result<VfsHostResponse> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(NativeClientError::InvalidState(
                "VFS control timeout must be positive and at most 300 seconds",
            ));
        }
        let deadline = Instant::now() + timeout;
        let mut stream = TcpStream::connect_timeout(
            &SocketAddr::V4(self.endpoint.address),
            timeout.min(Duration::from_secs(3)),
        )?;
        let mut stream = DeadlineStream {
            stream: &mut stream,
            deadline,
        };
        write_frame(
            &mut stream,
            &Request {
                protocol: PROTOCOL.into(),
                token: self.endpoint.token.clone(),
                command,
            },
        )?;
        Ok(read_frame(&mut stream)?)
    }
}

/// A socket timeout alone restarts after every successful short read. Recompute
/// the remaining budget for every syscall so a trickled prefix or payload cannot
/// retain the single host loop indefinitely. Clients use the same total budget.
struct DeadlineStream<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl DeadlineStream<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "VFS control frame deadline exceeded",
                )
            })
    }
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        let read = self.stream.read(bytes)?;
        self.remaining()?;
        Ok(read)
    }
}
impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        let written = self.stream.write(bytes)?;
        self.remaining()?;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.stream.flush()
    }
}

fn read_authenticated_request(
    stream: &mut TcpStream,
    token: &str,
    deadline: Instant,
) -> io::Result<Request> {
    let request: Request = read_frame(&mut DeadlineStream { stream, deadline })?;
    if request.protocol != PROTOCOL || !token_matches(&request.token, token) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "VFS control request rejected",
        ));
    }
    Ok(request)
}

fn json(value: impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| NativeClientError::Filesystem(io::Error::other(e)))
}

fn token_matches(provided: &str, expected: &str) -> bool {
    if provided.len() != 64 || expected.len() != 64 {
        return false;
    }
    provided
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |different, (a, b)| different | (a ^ b))
        == 0
}

fn write_frame(stream: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("VFS control response too large"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}
fn read_frame<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(io::Error::other("invalid VFS control frame length"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid VFS control frame"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_json_rejects_unknown_fields_including_status_and_stop() {
        for valid in [
            serde_json::json!({"action": "status"}),
            serde_json::json!({"action": "stop"}),
            serde_json::json!({"action": "capture", "writers_quiesced": true}),
            serde_json::json!({"action": "export", "parent": "export-parent", "writers_quiesced": true}),
        ] {
            let command: VfsHostCommand = serde_json::from_value(valid.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), valid);
            let mut invalid = valid;
            invalid
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), serde_json::json!({"nested": true}));
            assert!(serde_json::from_value::<VfsHostCommand>(invalid.clone()).is_err());
            // Exercise the actual framed Request shape as well as direct enum
            // parsing, so an ignored nested command field cannot enter serve.
            let mut frame = Vec::new();
            write_frame(
                &mut frame,
                &serde_json::json!({
                    "protocol": PROTOCOL, "token": "a".repeat(64), "command": invalid,
                }),
            )
            .unwrap();
            assert!(read_frame::<Request>(&mut frame.as_slice()).is_err());
        }
    }

    #[test]
    fn trickled_unauthenticated_frame_expires_and_next_authenticated_request_succeeds() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        let token = "a".repeat(64);
        let server_token = token.clone();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let start = Instant::now();
            accepted_tx.send(()).unwrap();
            assert!(read_authenticated_request(
                &mut first,
                &server_token,
                start + Duration::from_millis(200),
            )
            .is_err());
            // The sender supplies bytes faster than a relative read timeout,
            // across both the four-byte prefix and the payload.
            assert!(start.elapsed() < Duration::from_secs(1));
            drop(first);
            let (mut next, _) = listener.accept().unwrap();
            let request = read_authenticated_request(
                &mut next,
                &server_token,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
            assert!(matches!(request.command, VfsHostCommand::Status {}));
            write_frame(
                &mut next,
                &VfsHostResponse::Ok {
                    value: serde_json::json!({"available": true}),
                },
            )
            .unwrap();
        });
        let mut attacker = TcpStream::connect(address).unwrap();
        accepted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let attacker = std::thread::spawn(move || {
            let bytes = (4096u32)
                .to_be_bytes()
                .into_iter()
                .chain(std::iter::repeat(b' '));
            for byte in bytes.take(100) {
                if attacker.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let client = VfsHostClient {
            endpoint: Endpoint {
                protocol: PROTOCOL.into(),
                address,
                token,
                worktree_id: WorktreeId::parse("worktree:deadline-test").unwrap(),
            },
        };
        match client
            .request_with_timeout(VfsHostCommand::Status {}, Duration::from_secs(2))
            .unwrap()
        {
            VfsHostResponse::Ok { value } => assert_eq!(value["available"], true),
            response => panic!("authenticated request failed: {response:?}"),
        }
        server.join().unwrap();
        attacker.join().unwrap();
    }

    #[test]
    fn framing_rejects_oversize_before_reading_payload_and_auth_is_exact() {
        let mut prefix = ((MAX_FRAME + 1) as u32).to_be_bytes().as_slice().to_vec();
        assert!(read_frame::<Request>(&mut prefix.as_slice()).is_err());
        let token = "a".repeat(64);
        assert!(token_matches(&token, &token));
        assert!(!token_matches(&"a".repeat(63), &token));
        assert!(!token_matches(&format!("{}b", "a".repeat(63)), &token));
        prefix.clear();
        write_frame(&mut prefix, &VfsHostCommand::Status {}).unwrap();
        assert!(matches!(
            read_frame::<VfsHostCommand>(&mut prefix.as_slice()).unwrap(),
            VfsHostCommand::Status {}
        ));
    }
}
