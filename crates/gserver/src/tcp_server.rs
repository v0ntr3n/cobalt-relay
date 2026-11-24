//! Multi-threaded TCP AMF3 / `LCPacket` game server listener for GServer (`vnsg2`).

use std::io::{self, Read};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gserver_protocol::{C2sFrameMode, ClientRxDecoder, RpcRequest};

use crate::world::{ConnectionSession, ServerWorld};

/// Handle to a running TCP AMF3 game server listener.
pub struct TcpGameServerHandle {
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl TcpGameServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.local_addr);
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TcpGameServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawns the multi-threaded TCP game server accepting `LCPacket` (24B) & raw (4B) AMF3 streams.
pub fn spawn_tcp_game_server<A: ToSocketAddrs>(
    bind_addr: A,
    world: Arc<ServerWorld>,
) -> io::Result<TcpGameServerHandle> {
    let listener = TcpListener::bind(bind_addr)?;
    let local_addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = Arc::clone(&shutdown);

    let accept_thread = thread::Builder::new()
        .name("gserver-tcp-accept".into())
        .spawn(move || {
            while !shutdown_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
                        let world_clone = Arc::clone(&world);
                        let conn_shutdown = Arc::clone(&shutdown_flag);
                        let _ = thread::spawn(move || {
                            handle_client_connection(stream, world_clone, conn_shutdown);
                        });
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(25));
                    }
                }
            }
        })?;

    Ok(TcpGameServerHandle {
        local_addr,
        shutdown,
        accept_thread: Some(accept_thread),
    })
}

fn handle_client_connection(
    mut reader: TcpStream,
    world: Arc<ServerWorld>,
    shutdown: Arc<AtomicBool>,
) {
    let Ok(writer_stream) = reader.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(writer_stream));
    let mut rx = ClientRxDecoder::new(C2sFrameMode::Auto);
    let mut session = ConnectionSession::default();
    let mut buf = [0u8; 8192];

    while !shutdown.load(Ordering::SeqCst) {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                rx.feed(&buf[..n]);
                loop {
                    match rx.next_frame() {
                        Ok(Some(frame)) => {
                            let Ok(req) = RpcRequest::from_amf3_bytes(&frame.amf3_payload) else {
                                break;
                            };
                            let was_logged_in = session.logged_in_uid;
                            let handled = world.handle_rpc(&mut session, &req);

                            if was_logged_in.is_none() {
                                if let Some(uid) = session.logged_in_uid {
                                    world.bind_peer_stream(uid, Arc::clone(&writer));
                                }
                            }

                            let mut write_failed = false;
                            for env in &handled.caller_envelopes {
                                if world.write_envelope(&writer, env).is_err() {
                                    write_failed = true;
                                    break;
                                }
                            }
                            world.broadcast_peer_pushes(handled.peer_envelopes);

                            if write_failed {
                                world.on_disconnect(&session);
                                return;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => {
                            world.on_disconnect(&session);
                            return;
                        }
                    }
                }
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }

    world.on_disconnect(&session);
}
