use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slot_retro::LinkChannel;

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

const POLL_MS: u64 = 50;

pub const HOST_BOUND: Duration = Duration::from_secs(60);

const CONTROL_ENDED: u8 = 0x00;

const BYE_MS: u64 = 100;

enum Out {
    Packet(Vec<u8>),
    Control(u8, Sender<()>),
}

pub struct TcpLink {
    outbox: Sender<Out>,
    inbox: Receiver<Vec<u8>>,
    stream: TcpStream,
    closed: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
}

impl TcpLink {
    pub fn host_until(
        addr: &str,
        port: u16,
        bound: Duration,
        cancel: &Cancel,
    ) -> std::io::Result<TcpLink> {
        let listener = TcpListener::bind((addr, port)).map_err(|error| {
            crate::link_log::warn(format!("host bind failed addr={addr}:{port}: {error}"));
            error
        })?;
        listener.set_nonblocking(true)?;
        let local = listener.local_addr().ok();
        crate::link_log::debug(format!(
            "host listening addr={addr}:{port} local={local:?} bound={}s",
            bound.as_secs()
        ));
        let deadline = Instant::now() + bound;
        loop {
            if cancel.is_cancelled() {
                crate::link_log::debug("host cancelled while waiting for a peer");
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled while waiting for a peer",
                ));
            }
            match listener.accept() {
                Ok((stream, peer)) => {
                    crate::link_log::debug(format!("host accepted peer={peer}"));
                    stream.set_nonblocking(false)?;
                    return TcpLink::wrap(stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    crate::link_log::warn(format!("host accept failed addr={addr}:{port}: {e}"));
                    return Err(e);
                }
            }
            if Instant::now() >= deadline {
                crate::link_log::debug(format!(
                    "host timed out after {} ms with no peer",
                    bound.as_millis()
                ));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "no peer arrived",
                ));
            }
            std::thread::sleep(Duration::from_millis(POLL_MS));
        }
    }

    pub fn host(addr: &str, port: u16) -> std::io::Result<TcpLink> {
        TcpLink::host_until(addr, port, HOST_BOUND, &Cancel::new())
    }

    pub fn join_until(
        addr: &str,
        port: u16,
        bound: Duration,
        cancel: &Cancel,
    ) -> std::io::Result<TcpLink> {
        let deadline = Instant::now() + bound;
        crate::link_log::debug(format!(
            "join connecting addr={addr}:{port} bound={}s",
            bound.as_secs()
        ));
        let mut attempts = 0u32;
        loop {
            if cancel.is_cancelled() {
                crate::link_log::debug(format!(
                    "join cancelled after {attempts} attempts while reaching for the host"
                ));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled while reaching for the host",
                ));
            }
            attempts = attempts.saturating_add(1);
            match TcpStream::connect((addr, port)) {
                Ok(stream) => {
                    crate::link_log::debug(format!(
                        "join connected addr={addr}:{port} attempts={attempts}"
                    ));
                    return TcpLink::wrap(stream);
                }
                Err(error) => {
                    if attempts == 1 || attempts % 20 == 0 {
                        crate::link_log::debug(format!(
                            "join attempt {attempts} addr={addr}:{port} failed: {error} ({:?})",
                            error.kind()
                        ));
                    }
                }
            }
            if Instant::now() >= deadline {
                crate::link_log::debug(format!(
                    "join timed out after {} ms attempts={attempts}",
                    bound.as_millis()
                ));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "never reached the host",
                ));
            }
            std::thread::sleep(Duration::from_millis(POLL_MS));
        }
    }

    pub fn join(addr: &str, port: u16) -> std::io::Result<TcpLink> {
        TcpLink::wrap(TcpStream::connect((addr, port))?)
    }

    fn wrap(stream: TcpStream) -> std::io::Result<TcpLink> {
        let local = stream.local_addr().ok();
        let peer = stream.peer_addr().ok();
        stream.set_nodelay(true)?;
        crate::link_log::debug(format!(
            "tcp connected local={local:?} peer={peer:?} nodelay=true"
        ));
        let mut reader = stream.try_clone()?;
        let mut writer = stream.try_clone()?;
        let (rtx, inbox) = channel();
        let (wtx, wrx) = channel::<Out>();
        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = closed.clone();
        let ended = Arc::new(AtomicBool::new(false));
        let reader_ended = ended.clone();

        std::thread::spawn(move || {
            let mut header = [0u8; 2];
            let mut control = false;
            let (mut packets, mut bytes) = (0usize, 0usize);
            loop {
                if let Err(error) = reader.read_exact(&mut header) {
                    reader_closed.store(true, Ordering::Release);
                    if error.kind() == std::io::ErrorKind::UnexpectedEof {
                        crate::link_log::debug(format!(
                            "tcp reader EOF packets={packets} bytes={bytes}"
                        ));
                    } else {
                        crate::link_log::warn(format!(
                            "tcp reader failed packets={packets} bytes={bytes}: {error}"
                        ));
                    }
                    return;
                }
                let len = u16::from_be_bytes(header) as usize;
                if len == 0 && !control {
                    control = true;
                    continue;
                }
                let mut buf = vec![0u8; len];
                if len > 0 {
                    if let Err(error) = reader.read_exact(&mut buf) {
                        reader_closed.store(true, Ordering::Release);
                        crate::link_log::warn(format!(
                            "tcp reader body failed len={len} packets={packets}: {error}"
                        ));
                        return;
                    }
                }
                if std::mem::take(&mut control) {
                    let op = buf.first().copied().unwrap_or(0xff);
                    if op == CONTROL_ENDED {
                        reader_ended.store(true, Ordering::Release);
                    }
                    crate::link_log::debug(format!(
                        "tcp rx control op=0x{op:02x} ended={}",
                        op == CONTROL_ENDED
                    ));
                    continue;
                }
                packets = packets.saturating_add(1);
                bytes = bytes.saturating_add(len);
                if packets == 1 || packets % 256 == 0 {
                    crate::link_log::debug(format!(
                        "tcp rx packets={packets} bytes={bytes} last_len={len}"
                    ));
                }
                if rtx.send(buf).is_err() {
                    crate::link_log::debug(format!(
                        "tcp reader receiver dropped packets={packets} bytes={bytes}"
                    ));
                    return;
                }
            }
        });

        std::thread::spawn(move || {
            let (mut packets, mut bytes) = (0usize, 0usize);
            for out in wrx.iter() {
                match out {
                    Out::Packet(buf) => {
                        let Ok(len) = u16::try_from(buf.len()) else {
                            crate::link_log::warn(format!(
                                "tcp tx packet too large len={}",
                                buf.len()
                            ));
                            continue;
                        };
                        if let Err(error) = writer.write_all(&len.to_be_bytes()) {
                            crate::link_log::warn(format!(
                                "tcp writer header failed len={len} packets={packets}: {error}"
                            ));
                            return;
                        }
                        if let Err(error) = writer.write_all(&buf) {
                            crate::link_log::warn(format!(
                                "tcp writer body failed len={len} packets={packets}: {error}"
                            ));
                            return;
                        }
                        packets = packets.saturating_add(1);
                        bytes = bytes.saturating_add(len as usize);
                        if packets == 1 || packets % 256 == 0 {
                            crate::link_log::debug(format!(
                                "tcp tx packets={packets} bytes={bytes} last_len={len}"
                            ));
                        }
                    }
                    Out::Control(op, ack) => {
                        crate::link_log::debug(format!("tcp tx control op=0x{op:02x}"));
                        if let Err(error) = writer.write_all(&[0, 0, 0, 1, op]) {
                            crate::link_log::warn(format!(
                                "tcp writer control failed op=0x{op:02x} packets={packets}: {error}"
                            ));
                            return;
                        }
                        let _ = ack.send(());
                    }
                }
            }
            crate::link_log::debug(format!("tcp writer ended packets={packets} bytes={bytes}"));
        });

        Ok(TcpLink {
            outbox: wtx,
            inbox,
            stream,
            closed,
            ended,
        })
    }

    pub fn send_end(&mut self) {
        crate::link_log::debug("tcp send_end requested");
        let (ack, wrote) = channel();
        if self.outbox.send(Out::Control(CONTROL_ENDED, ack)).is_err() {
            crate::link_log::warn("tcp send_end: writer already gone");
            return;
        }
        match wrote.recv_timeout(Duration::from_millis(BYE_MS)) {
            Ok(()) => crate::link_log::debug("tcp send_end acknowledged"),
            Err(error) => crate::link_log::warn(format!("tcp send_end not acknowledged: {error}")),
        }
    }

    pub fn nodelay(&self) -> std::io::Result<bool> {
        self.stream.nodelay()
    }
}

impl Drop for TcpLink {
    fn drop(&mut self) {
        crate::link_log::debug(format!(
            "tcp close local={:?} peer={:?}",
            self.stream.local_addr().ok(),
            self.stream.peer_addr().ok()
        ));
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl LinkChannel for TcpLink {
    fn send(&mut self, _flags: i32, buf: &[u8]) {
        if buf.is_empty() {
            return;
        }
        let _ = self.outbox.send(Out::Packet(buf.to_vec()));
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        match self.inbox.try_recv() {
            Ok(p) => Some(p),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn send_end(&mut self) {
        TcpLink::send_end(self);
    }

    fn peer_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}
