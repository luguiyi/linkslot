use std::collections::BTreeMap;

use slot_retro::ButtonMask;

use crate::rom_id::Digest;

pub const DELAY: u64 = 3;

const PACKET: usize = 11;

const KIND_MASK: u8 = 0;
const KIND_STATE: u8 = 1;
const KIND_STATE_END: u8 = 2;
const KIND_READY: u8 = 3;
const KIND_HELLO: u8 = 4;
/// Session control, not cable traffic: the two ends talk about *which game* is
/// running while neither has a cable open. The worker forwards these to the
/// app instead of feeding them to the cable.
const KIND_GAME_START: u8 = 0x10;
const KIND_GAME_END: u8 = 0x11;
const KIND_GAME_REFUSE: u8 = 0x12;
const KIND_PING: u8 = 0x13;
const KIND_PONG: u8 = 0x14;
/// The host's clock, so the joiner's time matches the moment it joins.
const KIND_TIME_SYNC: u8 = 0x1b;
/// Why one end will not follow the other into a game.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FollowRefusal {
    /// The player said no.
    Declined,
    /// The cart is not on this card.
    MissingRom,
    /// Something local says "not now": a one-cart card, a read-only card, a
    /// write in flight.
    Busy,
}

impl FollowRefusal {
    pub fn code(self) -> u8 {
        match self {
            FollowRefusal::Declined => 0,
            FollowRefusal::MissingRom => 1,
            FollowRefusal::Busy => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<FollowRefusal> {
        Some(match code {
            0 => FollowRefusal::Declined,
            1 => FollowRefusal::MissingRom,
            2 => FollowRefusal::Busy,
            _ => return None,
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            FollowRefusal::Declined => "对方拒绝了切换",
            FollowRefusal::MissingRom => "对方没有这张卡",
            FollowRefusal::Busy => "对方现在无法切换",
        }
    }
}

/// Longest game name carried by [`GameStart`], in bytes.
pub const GAME_NAME_MAX: usize = 32;

/// What a game says when it starts, so the other end can compare without a
/// cable: same machine, same cart.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GameStart {
    pub platform: u8,
    pub core: u8,
    pub rom: Digest,
    pub name: String,
}

impl GameStart {
    /// Whether both ends are running the same cart on the same machine.
    pub fn agrees(&self, peer: &GameStart) -> bool {
        self.platform == peer.platform && self.core == peer.core && self.rom == peer.rom
    }
}

pub fn game_start_packet(start: &GameStart) -> Vec<u8> {
    let mut end = 0;
    for (at, ch) in start.name.trim().char_indices() {
        let next = at + ch.len_utf8();
        if next > GAME_NAME_MAX {
            break;
        }
        end = next;
    }
    let mut out = Vec::with_capacity(13 + end);
    out.push(KIND_GAME_START);
    out.push(start.platform);
    out.push(start.core);
    out.extend_from_slice(&start.rom);
    out.push(end as u8);
    out.extend_from_slice(&start.name.trim().as_bytes()[..end]);
    out
}

pub fn decode_game_start(buf: &[u8]) -> Option<GameStart> {
    // kind, platform, core, rom (32), name length, name: the length sits at
    // 35 and the name starts at 36.
    if buf.len() < 36 || buf[0] != KIND_GAME_START {
        return None;
    }
    let name_len = *buf.get(35)? as usize;
    let name = buf.get(36..36 + name_len)?;
    if buf.len() != 36 + name_len {
        return None;
    }
    Some(GameStart {
        platform: buf[1],
        core: buf[2],
        rom: buf[3..35].try_into().ok()?,
        name: String::from_utf8_lossy(name).trim().to_string(),
    })
}

pub fn game_end_packet() -> [u8; 1] {
    [KIND_GAME_END]
}

pub fn refuse_packet(reason: FollowRefusal) -> [u8; 2] {
    [KIND_GAME_REFUSE, reason.code()]
}

pub fn decode_refuse(buf: &[u8]) -> Option<FollowRefusal> {
    if buf.len() != 2 || buf[0] != KIND_GAME_REFUSE {
        return None;
    }
    FollowRefusal::from_code(buf[1])
}

pub fn ping_packet() -> [u8; 1] {
    [KIND_PING]
}

pub fn pong_packet() -> [u8; 1] {
    [KIND_PONG]
}

pub fn is_ping(buf: &[u8]) -> bool {
    buf.first() == Some(&KIND_PING)
}

pub fn is_pong(buf: &[u8]) -> bool {
    buf.first() == Some(&KIND_PONG)
}

/// Whether a frame belongs to the session layer rather than the cable.
pub fn is_control(buf: &[u8]) -> bool {
    matches!(
        buf.first(),
        Some(&KIND_GAME_START)
            | Some(&KIND_GAME_END)
            | Some(&KIND_GAME_REFUSE)
            | Some(&KIND_PING)
            | Some(&KIND_PONG)
            | Some(&KIND_TIME_SYNC)
    )
}

/// The host's clock, packed for the joiner: UTC seconds and the owner's offset.
pub fn time_sync_packet(utc: i64, offset_min: i16) -> [u8; 11] {
    let mut out = [0u8; 11];
    out[0] = KIND_TIME_SYNC;
    out[1..9].copy_from_slice(&utc.to_le_bytes());
    out[9..11].copy_from_slice(&offset_min.to_le_bytes());
    out
}

pub fn decode_time_sync(buf: &[u8]) -> Option<(i64, i16)> {
    if buf.len() != 11 || buf[0] != KIND_TIME_SYNC {
        return None;
    }
    Some((
        i64::from_le_bytes(buf[1..9].try_into().ok()?),
        i16::from_le_bytes(buf[9..11].try_into().ok()?),
    ))
}

const CHUNK: usize = 60_000;

/// The cable protocol's own version. Bumped when a packet's meaning changes;
/// the single-machine handshake (FC/SFC/MD) requires the two ends to agree on it.
pub const HELLO_VERSION: u8 = 2;

/// Bytes a device name may take in a hello. See `linkcfg::DEVICE_NAME_MAX`.
pub const HELLO_DEVICE: usize = 16;

/// What one end tells the other about itself before any save state moves.
///
/// The single-machine path (Famicom / Super Famicom / Mega Drive) has no BIOS
/// to compare
/// (the GBA pair's check, see `link_state::same_bios`) and no core that would
/// reject a foreign state: a console save state is a shapeless blob that the
/// core will happily write into whichever cart it is running. So the two ends
/// say what they are and what they are playing, and a mismatch is refused
/// before the first byte of state lands.
///
/// `platform` and `core` are the `slot-store` discriminants; `rom` is the
/// full SHA-256 of the cart file, so both ends can compare exact content.
///
/// `device` is only ever shown to the player ("对方：客厅机"); it takes no part
/// in `agrees`, so two devices may carry the same name without failing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Hello {
    pub version: u8,
    pub platform: u8,
    pub core: u8,
    pub rom: Digest,
    pub device: [u8; HELLO_DEVICE],
}

impl Hello {
    /// Whether this end and `peer` are in the same game on the same machine.
    pub fn agrees(&self, peer: &Hello) -> bool {
        self.version == peer.version
            && self.platform == peer.platform
            && self.core == peer.core
            && self.rom == peer.rom
    }

    /// The device name as text: trailing NULs dropped, and a name that was cut
    /// mid-character still renders (the lossy decode only loses that one).
    pub fn device_name(&self) -> String {
        let end = self
            .device
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(HELLO_DEVICE);
        String::from_utf8_lossy(&self.device[..end]).trim().to_string()
    }

    /// The same hello, labelled with `name` cut to the slot on a character
    /// boundary.
    pub fn named(mut self, name: &str) -> Hello {
        self.device = [0; HELLO_DEVICE];
        let mut end = 0;
        for (at, ch) in name.trim().char_indices() {
            let next = at + ch.len_utf8();
            if next > HELLO_DEVICE {
                break;
            }
            end = next;
        }
        self.device[..end].copy_from_slice(&name.trim().as_bytes()[..end]);
        self
    }
}

const HELLO_ROM_AT: usize = 4;
const HELLO_DEVICE_AT: usize = HELLO_ROM_AT + 32;
pub const HELLO_LEN: usize = HELLO_DEVICE_AT + HELLO_DEVICE;

pub fn hello_packet(hello: Hello) -> [u8; HELLO_LEN] {
    let mut out = [0u8; HELLO_LEN];
    out[0] = KIND_HELLO;
    out[1] = hello.version;
    out[2] = hello.platform;
    out[3] = hello.core;
    out[HELLO_ROM_AT..HELLO_DEVICE_AT].copy_from_slice(&hello.rom);
    out[HELLO_DEVICE_AT..].copy_from_slice(&hello.device);
    out
}

pub fn decode_hello(buf: &[u8]) -> Option<Hello> {
    if buf.len() != HELLO_LEN || buf[0] != KIND_HELLO {
        return None;
    }
    let mut device = [0u8; HELLO_DEVICE];
    device.copy_from_slice(&buf[HELLO_DEVICE_AT..]);
    Some(Hello {
        version: buf[1],
        platform: buf[2],
        core: buf[3],
        rom: buf[HELLO_ROM_AT..HELLO_DEVICE_AT].try_into().ok()?,
        device,
    })
}

/// The single machine's opening transfer (Famicom / Super Famicom): one
/// machine's state *and* its battery save.
///
/// A save state does not carry SRAM, so two devices whose `.sav` files differ
/// would start the same game from the same frame with different save data
/// behind it — a divergence that shows up minutes later, as a desync. Sending
/// the SRAM too is a few kilobytes and removes the whole class.
pub fn single_payload(state: &[u8], sav: Option<&[u8]>) -> Vec<u8> {
    let sav = sav.unwrap_or(&[]);
    let mut out = Vec::with_capacity(8 + state.len() + sav.len());
    out.extend_from_slice(&(state.len() as u32).to_le_bytes());
    out.extend_from_slice(state);
    out.extend_from_slice(&(sav.len() as u32).to_le_bytes());
    out.extend_from_slice(sav);
    out
}

/// Read back what [`single_payload`] wrote. `None` for anything that is not
/// exactly that shape — a peer that never sent one, or a truncated transfer.
pub fn decode_single_payload(buf: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let state_len = u32::from_le_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let state = buf.get(4..4 + state_len)?.to_vec();
    let rest = buf.get(4 + state_len..)?;
    let sav_len = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let sav = rest.get(4..4 + sav_len)?.to_vec();
    if rest.len() != 4 + sav_len {
        return None;
    }
    Some((state, (!sav.is_empty()).then_some(sav)))
}

pub fn encode(frame: u64, mask: ButtonMask) -> [u8; PACKET] {
    let mut out = [0u8; PACKET];
    out[0] = KIND_MASK;
    out[1..9].copy_from_slice(&frame.to_le_bytes());
    out[9..].copy_from_slice(&mask.0.to_le_bytes());
    out
}

pub fn decode(buf: &[u8]) -> Option<(u64, ButtonMask)> {
    if buf.len() != PACKET || buf[0] != KIND_MASK {
        return None;
    }
    let frame = u64::from_le_bytes(buf[1..9].try_into().ok()?);
    let mask = u16::from_le_bytes(buf[9..].try_into().ok()?);
    Some((frame, ButtonMask(mask)))
}

pub fn ready_packet() -> [u8; 1] {
    [KIND_READY]
}

pub fn state_packets(state: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = state
        .chunks(CHUNK)
        .map(|c| {
            let mut p = Vec::with_capacity(c.len() + 1);
            p.push(KIND_STATE);
            p.extend_from_slice(c);
            p
        })
        .collect();
    match out.last_mut() {
        Some(last) => last[0] = KIND_STATE_END,
        None => out.push(vec![KIND_STATE_END]),
    }
    out
}

/// How the two ends of a cable link are shaped.
///
/// The Advance's link mode runs **two** GBAs inside one core and serializes
/// them as a pair, so its save states are wrapped per player and the two ends
/// have to agree on the BIOS. A Famicom, Super Famicom or Mega Drive is
/// **one** machine with two pads: its state is the whole truth, nothing is
/// paired, and the identity of the game is the only thing worth comparing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Pairing {
    /// mGBA's two-GBA link: paired states, BIOS check on the joiner.
    PairedGba,
    /// One machine, two pads (FC/SFC/MD): state plus SRAM, gated on a hello.
    SingleMachine(Hello),
}

pub struct Cable {
    player: u8,
    pairing: Pairing,
    frame: u64,
    local: BTreeMap<u64, ButtonMask>,
    remote: BTreeMap<u64, ButtonMask>,
    stalled: u32,
    primed: bool,
    incoming: Vec<u8>,
    complete: bool,
    peer_ready: bool,
    peer_hello: Option<Hello>,
}

impl Cable {
    pub fn new(player: u8, pairing: Pairing) -> Self {
        let idle = ButtonMask::default();
        let seed: BTreeMap<u64, ButtonMask> = (0..DELAY).map(|f| (f, idle)).collect();
        Cable {
            player,
            pairing,
            frame: 0,
            local: seed.clone(),
            remote: seed,
            stalled: 0,
            primed: player == 0,
            incoming: Vec::new(),
            complete: false,
            peer_ready: player != 0,
            peer_hello: None,
        }
    }

    pub fn pairing(&self) -> Pairing {
        self.pairing
    }

    /// What the other end said it was, once its hello has landed.
    pub fn peer_hello(&self) -> Option<Hello> {
        self.peer_hello
    }

    /// Whether frames may be stepped at all.
    ///
    /// A Famicom/Super Famicom/Mega Drive pair waits for the other end to say
    /// it is the same game: the seeded idle frames are enough for `ready()` to
    /// answer for the first `DELAY` frames, and running them against a cart
    /// that turns out to be a different game is exactly the thing the hello
    /// exists to prevent.
    /// The mGBA pair has no such gate — its BIOS check happens on the state.
    pub fn may_run(&self) -> bool {
        match self.pairing {
            Pairing::PairedGba => true,
            Pairing::SingleMachine(mine) => self.peer_hello.is_some_and(|peer| mine.agrees(&peer)),
        }
    }

    pub fn primed(&self) -> bool {
        self.primed
    }

    pub fn armed(&self) -> bool {
        self.primed && self.peer_ready
    }

    pub fn peer_ready(&self) -> bool {
        self.peer_ready
    }

    pub fn take_state(&mut self) -> Option<Vec<u8>> {
        if !self.complete {
            return None;
        }
        self.complete = false;
        Some(std::mem::take(&mut self.incoming))
    }

    pub fn prime(&mut self) {
        self.primed = true;
    }

    pub fn player(&self) -> u8 {
        self.player
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    pub fn stalled(&self) -> u32 {
        self.stalled
    }

    /// A mask we still hold for `frame`, for resending to a peer that is a
    /// little behind: the seeded delay window is the only slack a lockstep
    /// pair has.
    pub fn peek_mask(&self, frame: u64) -> Option<[u8; PACKET]> {
        self.local.get(&frame).map(|mask| encode(frame, *mask))
    }

    /// Frame numbers of the masks held for each side, for the stall log.
    pub fn mask_frames(&self) -> (Vec<u64>, Vec<u64>) {
        (
            self.local.keys().copied().collect(),
            self.remote.keys().copied().collect(),
        )
    }

    pub fn sample(&mut self, mask: ButtonMask) -> [u8; PACKET] {
        let frame = self.frame + DELAY;
        let mask = *self.local.entry(frame).or_insert(mask);
        encode(frame, mask)
    }

    pub fn accept(&mut self, buf: &[u8]) -> bool {
        match buf.first() {
            Some(&KIND_READY) => {
                self.peer_ready = true;
                return true;
            }
            Some(&KIND_HELLO) => {
                // A hello is the peer's identity, not a state packet and not a
                // "I have restored your state" — `peer_ready` is that, and only
                // READY sets it. A hello that will not parse is ignored rather
                // than fatal, so a peer from before this packet still works.
                if let Some(hello) = decode_hello(buf) {
                    self.peer_hello = Some(hello);
                    return true;
                }
                return false;
            }
            Some(&KIND_STATE) => {
                self.incoming.extend_from_slice(&buf[1..]);
                return true;
            }
            Some(&KIND_STATE_END) => {
                self.incoming.extend_from_slice(&buf[1..]);
                self.complete = true;
                return true;
            }
            _ => {}
        }
        let Some((frame, mask)) = decode(buf) else {
            return false;
        };
        if frame < self.frame {
            return false;
        }
        self.remote.insert(frame, mask);
        true
    }

    pub fn ready(&self) -> Option<(ButtonMask, ButtonMask)> {
        if !self.primed {
            return None;
        }
        let mine = *self.local.get(&self.frame)?;
        let theirs = *self.remote.get(&self.frame)?;
        match self.player {
            0 => Some((mine, theirs)),
            _ => Some((theirs, mine)),
        }
    }

    pub fn advance(&mut self) {
        self.local.remove(&self.frame);
        self.remote.remove(&self.frame);
        self.frame += 1;
        self.stalled = 0;
    }

    pub fn stall(&mut self) {
        self.stalled = self.stalled.saturating_add(1);
    }
}

/// Frames of blocked lockstep before the cable calls the pair lost. Three
/// seconds, not one: a peer may be loading its core or between carts, and a
/// session keeps the socket anyway, so patience is cheaper than a false
/// disconnect.
pub const QUIET_FRAMES: u32 = 180;
