use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Instant;

use crate::link_net::{Cancel, TcpLink, HOST_BOUND};
use crate::link_radio::{self, LinkRole, RadioFail};

#[cfg(feature = "device")]
pub const HOST_ADDR: &str = "10.42.0.1";

#[cfg(not(feature = "device"))]
pub const HOST_ADDR: &str = "127.0.0.1";

pub const DEFAULT_LINK_PORT: u16 = 7211;

const PORT_ENV: &str = "SLOT_LINK_PORT";

pub fn link_port() -> u16 {
    let Some(raw) = std::env::var_os(PORT_ENV) else {
        return DEFAULT_LINK_PORT;
    };
    if raw.to_str().is_some_and(|s| s.trim().is_empty()) {
        return DEFAULT_LINK_PORT;
    }
    match raw.to_str().and_then(|s| s.trim().parse::<u16>().ok()) {
        None | Some(0) => {
            eprintln!(
                "slot: {PORT_ENV}={:?} is not a port a link can meet on, using {DEFAULT_LINK_PORT}",
                raw.to_string_lossy()
            );
            DEFAULT_LINK_PORT
        }
        Some(port) => port,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStep {
    Radio,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFail {
    Radio,
    NobodyCame,
    PeerVanished,
    Cancelled,
    /// The two ends are not the same machine, core or game. Only a Famicom
    /// link can say this: its save state has no BIOS header to disagree
    /// about, so the exchange is gated on the two ends' own identities.
    DifferentGame,
    /// The host started a game this device has no cart for.
    MissingRom,
    /// The other end said it has no cart for the game this one started.
    PeerMissingRom,
    /// The other end said no to switching.
    PeerDeclined,
    /// The other end cannot switch right now (read-only card, write in
    /// flight, one-cart card).
    PeerBusy,
}

impl LinkStep {
    pub const ALL: [LinkStep; 2] = [LinkStep::Radio, LinkStep::Waiting];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn line(self) -> &'static str {
        match self {
            LinkStep::Radio => "正在启动无线网络",
            LinkStep::Waiting => "正在寻找另一位玩家",
        }
    }

    pub fn shown(self, warm: bool) -> LinkStep {
        match self {
            LinkStep::Radio if warm => LinkStep::Waiting,
            step => step,
        }
    }
}

impl LinkFail {
    pub const SHOWN: [LinkFail; 7] = [
        LinkFail::Radio,
        LinkFail::NobodyCame,
        LinkFail::PeerVanished,
        LinkFail::DifferentGame,
        LinkFail::MissingRom,
        LinkFail::PeerMissingRom,
        LinkFail::PeerDeclined,
    ];

    pub fn shown(self) -> Option<usize> {
        LinkFail::SHOWN.iter().position(|f| *f == self)
    }

    pub fn line(self) -> &'static str {
        match self {
            LinkFail::Radio => "无线网络启动失败",
            LinkFail::NobodyCame => "未找到其他玩家",
            LinkFail::PeerVanished => "对方已断开连接",
            LinkFail::Cancelled => "已取消",
            LinkFail::DifferentGame => "对方的游戏不一致",
            LinkFail::MissingRom => "本机没有主机那张卡",
            LinkFail::PeerMissingRom => "对方没有这张卡",
            LinkFail::PeerDeclined => "对方没有切换",
            LinkFail::PeerBusy => "对方现在无法切换",
        }
    }
}

pub enum LinkProgress {
    At(LinkStep),
    Ready(TcpLink),
    Failed(LinkFail),
}

fn classify(e: &std::io::Error) -> LinkFail {
    match e.kind() {
        std::io::ErrorKind::Interrupted => LinkFail::Cancelled,
        std::io::ErrorKind::TimedOut => LinkFail::NobodyCame,
        _ => LinkFail::PeerVanished,
    }
}

type RadioUp = Box<dyn FnMut(LinkRole, &Cancel) -> Result<(), RadioFail> + Send>;
type RadioDown = Box<dyn FnMut() + Send>;
type Socket = Box<dyn FnMut(u16, &Cancel) -> std::io::Result<TcpLink> + Send>;

pub struct LinkStarter {
    rx: Receiver<LinkProgress>,
    cancel: Cancel,
    done: bool,
}

impl LinkStarter {
    pub fn spawn(role: LinkRole, port: u16) -> LinkStarter {
        LinkStarter::spawn_with(
            Box::new(link_radio::up),
            Box::new(link_radio::down),
            role,
            port,
            Box::new(move |port, cancel| match role {
                LinkRole::Host => TcpLink::host_until(HOST_ADDR, port, HOST_BOUND, cancel),
                LinkRole::Join => TcpLink::join_until(HOST_ADDR, port, HOST_BOUND, cancel),
            }),
        )
    }

    pub fn spawn_with(
        mut radio_up: RadioUp,
        mut radio_down: RadioDown,
        role: LinkRole,
        port: u16,
        mut socket: Socket,
    ) -> LinkStarter {
        let (tx, rx) = channel();
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            crate::link_log::debug(format!(
                "start role={:?} addr={HOST_ADDR} port={port} radio_bound={}s",
                role,
                HOST_BOUND.as_secs()
            ));
            let _ = tx.send(LinkProgress::At(LinkStep::Radio));
            if let Err(e) = radio_up(role, &flag) {
                let fail = match &e {
                    RadioFail::NoHost => LinkFail::NobodyCame,
                    RadioFail::Cancelled => LinkFail::Cancelled,
                    RadioFail::Radio(why) => {
                        crate::link_log::warn(format!(
                            "radio failed role={role:?} after {} ms: {why}",
                            started.elapsed().as_millis()
                        ));
                        LinkFail::Radio
                    }
                };
                if !matches!(e, RadioFail::Radio(_)) {
                    crate::link_log::debug(format!(
                        "radio stopped role={role:?}: {fail:?} after {} ms",
                        started.elapsed().as_millis()
                    ));
                }
                radio_down();
                let _ = tx.send(LinkProgress::Failed(fail));
                return;
            }
            let _ = tx.send(LinkProgress::At(LinkStep::Waiting));
            crate::link_log::debug(format!(
                "waiting role={role:?} addr={HOST_ADDR}:{port} after {} ms",
                started.elapsed().as_millis()
            ));
            match socket(port, &flag) {
                Ok(link) => {
                    crate::link_log::debug(format!(
                        "peer connected role={role:?} addr={HOST_ADDR}:{port} after {} ms",
                        started.elapsed().as_millis()
                    ));
                    if tx.send(LinkProgress::Ready(link)).is_err() {
                        crate::link_log::warn(format!(
                            "peer connected role={role:?} but nobody left to hand it to"
                        ));
                        radio_down();
                    }
                }
                Err(e) => {
                    let fail = classify(&e);
                    crate::link_log::warn(format!(
                        "socket failed role={role:?} addr={HOST_ADDR}:{port} after {} ms: {e} ({:?}) => {fail:?}",
                        started.elapsed().as_millis(),
                        e.kind()
                    ));
                    radio_down();
                    let _ = tx.send(LinkProgress::Failed(fail));
                }
            }
        });
        LinkStarter {
            rx,
            cancel,
            done: false,
        }
    }

    pub fn poll(&mut self) -> Option<LinkProgress> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(progress) => {
                self.done = matches!(progress, LinkProgress::Ready(_) | LinkProgress::Failed(_));
                Some(progress)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                crate::link_log::warn(
                    "link starter channel disconnected; treating as peer vanished",
                );
                Some(LinkProgress::Failed(LinkFail::PeerVanished))
            }
        }
    }

    pub fn cancel(&mut self) {
        crate::link_log::debug("cancel requested");
        self.cancel.cancel();
    }
}

impl Drop for LinkStarter {
    fn drop(&mut self) {
        if !self.done {
            crate::link_log::debug("starter dropped before it finished; cancelling");
        }
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_slow_join_has_enough_time_to_reach_the_host() {
        assert_eq!(HOST_BOUND, std::time::Duration::from_secs(60));
    }

    #[test]
    fn unset_means_the_number_both_devices_already_agree_on() {
        let _g = lock();
        std::env::remove_var(PORT_ENV);
        assert_eq!(link_port(), DEFAULT_LINK_PORT);
    }

    #[test]
    fn a_port_in_the_environment_wins() {
        let _g = lock();
        std::env::set_var(PORT_ENV, "7300");
        assert_eq!(link_port(), 7300);
        std::env::remove_var(PORT_ENV);
    }

    #[test]
    fn a_padded_port_is_still_a_port() {
        let _g = lock();
        std::env::set_var(PORT_ENV, "  7300 ");
        assert_eq!(link_port(), 7300);
        std::env::remove_var(PORT_ENV);
    }

    #[test]
    fn a_value_that_is_not_a_port_falls_back() {
        let _g = lock();
        for bad in ["banana", "-1", "70000", "7300x"] {
            std::env::set_var(PORT_ENV, bad);
            assert_eq!(
                link_port(),
                DEFAULT_LINK_PORT,
                "{bad:?} should not be taken"
            );
        }
        std::env::remove_var(PORT_ENV);
    }

    #[test]
    fn an_empty_value_is_an_unset_value() {
        let _g = lock();
        std::env::set_var(PORT_ENV, "   ");
        assert_eq!(link_port(), DEFAULT_LINK_PORT);
        std::env::remove_var(PORT_ENV);
    }

    #[test]
    fn zero_is_refused_even_though_it_parses() {
        let _g = lock();
        std::env::set_var(PORT_ENV, "0");
        assert_eq!(link_port(), DEFAULT_LINK_PORT);
        std::env::remove_var(PORT_ENV);
    }
}

#[cfg(test)]
mod step_tests {
    use super::*;

    #[test]
    fn a_warm_radio_captions_the_first_step_as_the_search() {
        assert_eq!(LinkStep::Radio.shown(true), LinkStep::Waiting);
        assert_eq!(
            LinkStep::Radio.shown(true).line(),
            "正在寻找另一位玩家"
        );
    }

    #[test]
    fn a_cold_radio_still_says_it_is_bringing_the_radio_up() {
        assert_eq!(LinkStep::Radio.shown(false), LinkStep::Radio);
        assert_eq!(LinkStep::Radio.shown(false).line(), "正在启动无线网络");
    }

    #[test]
    fn the_socket_step_says_the_same_thing_in_both_states() {
        assert_eq!(LinkStep::Waiting.shown(true), LinkStep::Waiting);
        assert_eq!(LinkStep::Waiting.shown(false), LinkStep::Waiting);
    }

    #[test]
    fn every_sentence_a_step_can_show_is_still_one_of_the_faces() {
        for warm in [true, false] {
            for step in LinkStep::ALL {
                assert!(
                    LinkStep::ALL.contains(&step.shown(warm)),
                    "{step:?} at warm={warm} shows a sentence with no face uploaded for it"
                );
            }
        }
    }
}
