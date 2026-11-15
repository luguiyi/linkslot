//! Cartridge-to-cartridge transfer mode.
//!
//! The ROM and the cartridge's own side files (cover label, intro text/video,
//! shell choice) move to the other device.  Battery saves, resume states,
//! rewind data and playtime stay where they were: those are the *game's* save
//! data, not the cartridge itself.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use slot_retro::LinkChannel;
use slot_store::{Cart, Outline, Platform, ShellChoice, ShellFinish};
use crate::rom_id::{self, Digest};

use crate::link_radio::LinkRole;
use crate::link_start::{LinkFail, LinkProgress, LinkStarter};

const MAGIC: &[u8; 4] = b"SCX4";
const KIND_OFFER: u8 = 1;
const KIND_ACCEPT: u8 = 2;
const KIND_REJECT: u8 = 3;
const KIND_DATA: u8 = 4;
const KIND_DONE: u8 = 5;
const KIND_ACK: u8 = 6;
const KIND_CANCEL: u8 = 7;
const KIND_DUPLICATE: u8 = 8;
const CHUNK: usize = 32 * 1024;

const ASSET_ROM: u8 = 0;
const ASSET_LABEL: u8 = 1;
const ASSET_INTRO: u8 = 2;
const ASSET_VIDEO: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Connecting,
    Shelf,
    OfferSent,
    Incoming,
    /// Same ROM already exists locally, but some side assets are missing.
    IncomingMeta,
    Sending,
    Receiving,
    Duplicate,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Offer {
    pub platform: Platform,
    pub extension: String,
    pub name: String,
    pub size: u64,
    /// Full SHA-256 of the source ROM.
    pub hash: Digest,
    pub assets: Vec<AssetMeta>,
    pub shell: Option<ShellChoice>,
}

impl Offer {
    /// Side assets the local cart with the same ROM is missing.  The ROM
    /// itself is never part of a metadata-only transfer.
    pub fn metadata_missing(&self, root: &Path, cart: &Cart) -> Vec<u8> {
        let labels = root.join("Labels").join(cart.platform.dir_name());
        let info = root.join("Info").join(cart.platform.dir_name());
        let mut missing = Vec::new();
        for asset in &self.assets {
            let present = match asset.kind {
                ASSET_LABEL => cart
                    .label
                    .as_ref()
                    .is_some_and(|path| path.is_file())
                    || labels.join(format!("{}.png", cart.stem)).is_file(),
                ASSET_INTRO => info.join(format!("{}.txt", cart.stem)).is_file(),
                ASSET_VIDEO => info.join(format!("{}.mp4", cart.stem)).is_file(),
                _ => true,
            };
            if !present && asset.size > 0 {
                missing.push(asset.kind);
            }
        }
        missing
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AssetMeta {
    pub kind: u8,
    pub size: u64,
}

#[derive(Clone, Debug)]
struct SourceAsset {
    meta: AssetMeta,
    path: PathBuf,
}

struct Selected {
    offer: Offer,
    assets: Vec<SourceAsset>,
}

struct IncomingAsset {
    meta: AssetMeta,
    temp: PathBuf,
    target: PathBuf,
    file: File,
    received: u64,
}

pub struct Transfer {
    pub host: bool,
    pub stage: Stage,
    starter: Option<LinkStarter>,
    wire: Option<Box<dyn LinkChannel>>,
    selected: Option<Selected>,
    pub incoming: Option<Offer>,
    pub progress: f32,
    pub peer: Option<String>,
    pub received: bool,
    pub duplicate_receiver: bool,
    /// Asset kinds the receiver asked for when the ROM is already local.
    pub metadata_missing: Vec<u8>,
    pub metadata_only: bool,
    metadata_stem: Option<String>,
    pub error: Option<String>,
    sending_index: usize,
    sending_at: u64,
    receiving: Vec<IncomingAsset>,
    receiving_total: u64,
    received_total: u64,
    target_stem: Option<String>,
    pub done_processed: bool,
}

impl Transfer {
    pub fn start(host: bool, port: u16) -> Transfer {
        let role = if host { LinkRole::Host } else { LinkRole::Join };
        Transfer {
            host,
            stage: Stage::Connecting,
            starter: Some(LinkStarter::spawn(role, port)),
            wire: None,
            selected: None,
            incoming: None,
            progress: 0.0,
            peer: None,
            received: false,
            duplicate_receiver: false,
            metadata_missing: Vec::new(),
            metadata_only: false,
            metadata_stem: None,
            error: None,
            sending_index: 0,
            sending_at: 0,
            receiving: Vec::new(),
            receiving_total: 0,
            received_total: 0,
            target_stem: None,
            done_processed: false,
        }
    }

    pub fn cancel(&mut self) {
        if let Some(starter) = &mut self.starter {
            starter.cancel();
        }
        if let Some(wire) = self.wire.as_mut() {
            wire.send(0, &message(KIND_CANCEL, &[]));
            wire.send_end();
        }
        self.abort_receive();
    }

    /// One frame of matching/transport upkeep.  `root` is only used by the
    /// receiver when it accepts an offer.
    pub fn update(&mut self, root: &Path) {
        if let Some(starter) = &mut self.starter {
            match starter.poll() {
                None => return,
                Some(LinkProgress::At(_)) => return,
                Some(LinkProgress::Ready(wire)) => {
                    self.wire = Some(Box::new(wire));
                    self.starter = None;
                    self.stage = Stage::Shelf;
                    crate::link_log::debug("transfer: transport matched");
                }
                Some(LinkProgress::Failed(fail)) => {
                    self.starter = None;
                    self.stage = Stage::Failed;
                    self.error = Some(match fail {
                        LinkFail::Cancelled => "已取消".into(),
                        LinkFail::NobodyCame => "没有找到另一台掌机".into(),
                        LinkFail::PeerVanished => "对方已断开".into(),
                        other => format!("匹配失败：{}", other.line()),
                    });
                }
            }
        }
        // Drain first: the receiver may send KIND_DUPLICATE and close in the
        // same tick.  Checking is_closed before try_recv would turn that
        // explicit refusal into a generic "对方已断开" and lose the reason.
        let mut frames = Vec::new();
        if let Some(wire) = self.wire.as_mut() {
            while let Some(frame) = wire.try_recv() {
                frames.push(frame);
            }
        }
        for frame in frames {
            self.handle_frame(root, &frame);
        }
        let closed = self.wire.as_ref().is_some_and(|wire| wire.is_closed());
        if closed
            && !matches!(
                self.stage,
                Stage::Shelf | Stage::Duplicate | Stage::Done
            )
        {
            self.stage = Stage::Failed;
            self.error = Some("对方已断开".into());
            self.abort_receive();
            return;
        }
        if self.stage == Stage::Sending {
            let mut wire = self.wire.take();
            if let Some(wire) = wire.as_mut() {
                self.pump_send(wire);
            }
            self.wire = wire;
        }
    }

    /// Asset labels for the transfer overlay.  Metadata-only transfers show
    /// only the missing side files, never ROM.
    pub fn display_assets(&self) -> Vec<&'static str> {
        let kinds: Vec<u8> = if !self.metadata_missing.is_empty() {
            self.metadata_missing.clone()
        } else if let Some(selected) = &self.selected {
            selected.assets.iter().map(|asset| asset.meta.kind).collect()
        } else if let Some(offer) = &self.incoming {
            offer.assets.iter().map(|asset| asset.kind).collect()
        } else {
            Vec::new()
        };
        kinds.into_iter().map(asset_label).collect()
    }

    pub fn select(&mut self, root: &Path, cart: &Cart, hash: Digest) -> Result<(), String> {
        if self.stage != Stage::Shelf || self.wire.is_none() {
            return Err("对方还没有就绪".into());
        }
        let extension = cart
            .rom
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| "这张卡没有可传输的扩展名".to_string())?
            .to_ascii_lowercase();
        if !cart.platform.extensions().contains(&extension.as_str()) {
            return Err("不支持这张卡的格式".into());
        }
        let assets = source_assets(root, cart)?;
        let size = assets.iter().map(|asset| asset.meta.size).sum::<u64>();
        let shell = cart.shell.or_else(|| {
            let effective = slot_ui::shell_for(cart);
            Some(ShellChoice {
                outline: Outline::Auto,
                colour: effective.colour,
                finish: match effective.finish {
                    slot_ui::Finish::Solid => ShellFinish::Solid,
                    slot_ui::Finish::Translucent => ShellFinish::Clear,
                    slot_ui::Finish::Glitter => ShellFinish::Glitter,
                },
            })
        });
        let offer = Offer {
            platform: cart.platform,
            extension,
            name: cart.stem.clone(),
            size,
            hash,
            assets: assets.iter().map(|asset| asset.meta).collect(),
            shell,
        };
        self.send_offer(&offer)?;
        self.selected = Some(Selected { offer, assets });
        self.stage = Stage::OfferSent;
        self.sending_index = 0;
        self.sending_at = 0;
        self.progress = 0.0;
        Ok(())
    }

    pub fn accept(&mut self, root: &Path) -> Result<(), String> {
        let offer = self
            .incoming
            .take()
            .ok_or_else(|| "没有待接收的卡带".to_string())?;
        if offer.size > 64 * 1024 * 1024 {
            return Err("卡带超过 64MB 上限".into());
        }
        if !offer
            .platform
            .extensions()
            .contains(&offer.extension.as_str())
            || !offer
                .extension
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err("对方发来的卡带格式不受支持".into());
        }
        if offer.assets.is_empty() || !offer.assets.iter().any(|a| a.kind == ASSET_ROM) {
            return Err("对方没有发送 ROM".into());
        }
        let stem = unique_stem(root, offer.platform, &offer.name, &offer.extension);
        let mut receiving = Vec::new();
        for asset in &offer.assets {
            let (temp, target) = asset_paths(root, offer.platform, &stem, &offer.extension, asset.kind)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录失败：{e}"))?;
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| format!("创建临时卡带文件失败：{e}"))?;
            receiving.push(IncomingAsset {
                meta: *asset,
                temp,
                target,
                file,
                received: 0,
            });
        }
        self.receiving_total = offer.size;
        self.received_total = 0;
        self.receiving = receiving;
        self.received = true;
        self.target_stem = Some(stem);
        self.incoming = Some(offer.clone());
        self.stage = Stage::Receiving;
        self.progress = 0.0;
        self.send_message(KIND_ACCEPT);
        Ok(())
    }

    /// The receiver found the same ROM but the offer carries side assets the
    /// local cart does not have.  Do not copy the ROM; ask the sender for the
    /// missing kinds only once the player confirms.
    pub fn offer_metadata(&mut self, stem: String, missing: Vec<u8>) {
        if self.stage != Stage::Incoming || missing.is_empty() {
            return;
        }
        self.metadata_stem = Some(stem);
        self.metadata_missing = missing;
        self.stage = Stage::IncomingMeta;
    }

    pub fn accept_metadata(&mut self, root: &Path) -> Result<(), String> {
        let offer = self
            .incoming
            .take()
            .ok_or_else(|| "没有待补充的卡带资料".to_string())?;
        let stem = self
            .metadata_stem
            .clone()
            .ok_or_else(|| "没有本地卡带目标".to_string())?;
        let missing = self.metadata_missing.clone();
        if missing.is_empty() {
            return Err("没有缺失的资料".to_string());
        }
        let mut mask = 0u8;
        let mut assets = Vec::new();
        for asset in &offer.assets {
            let wanted = missing.contains(&asset.kind) && asset.kind != ASSET_ROM;
            if wanted {
                mask |= 1u8 << asset.kind;
                assets.push(*asset);
            }
        }
        if mask == 0 {
            return Err("对方没有可补充的资料".to_string());
        }
        let total = assets.iter().map(|asset| asset.size).sum::<u64>();
        if total > 64 * 1024 * 1024 {
            return Err("补充资料超过 64MB 上限".into());
        }
        let mut receiving = Vec::new();
        for asset in &assets {
            let (temp, target) =
                asset_paths(root, offer.platform, &stem, &offer.extension, asset.kind)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建目录失败：{error}"))?;
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| format!("创建临时资料文件失败：{error}"))?;
            receiving.push(IncomingAsset {
                meta: *asset,
                temp,
                target,
                file,
                received: 0,
            });
        }
        self.receiving_total = total;
        self.received_total = 0;
        self.receiving = receiving;
        self.received = true;
        self.metadata_only = true;
        self.target_stem = Some(stem);
        self.incoming = Some(offer);
        self.stage = Stage::Receiving;
        self.progress = 0.0;
        self.send_payload(KIND_ACCEPT, &[mask]);
        Ok(())
    }

    pub fn reject(&mut self) {
        self.incoming = None;
        self.stage = Stage::Shelf;
        self.send_message(KIND_REJECT);
    }

    pub fn cancel_offer(&mut self) {
        if self.stage == Stage::OfferSent {
            self.send_message(KIND_CANCEL);
            self.selected = None;
            self.stage = Stage::Shelf;
            self.progress = 0.0;
        }
    }

    pub fn display_name(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|selected| selected.offer.name.as_str())
            .or_else(|| self.incoming.as_ref().map(|offer| offer.name.as_str()))
    }

    /// Final on-disk stem chosen by the receiver (may have a collision suffix).
    pub fn target_stem(&self) -> Option<&str> {
        self.target_stem.as_deref()
    }

    pub fn reset_after_done(&mut self) {
        if self.stage == Stage::Done {
            self.selected = None;
            self.incoming = None;
            self.progress = 0.0;
            self.stage = Stage::Shelf;
            self.done_processed = false;
        }
    }

    /// The App found the offered hash in its precomputed local index.  The
    /// receiver sends this before asking the player to accept anything.
    pub fn mark_duplicate(&mut self, receiver: bool) {
        self.error = Some(if receiver {
            "本机已拥有当前卡带".into()
        } else {
            "对方已拥有当前卡带".into()
        });
        self.duplicate_receiver = receiver;
        self.stage = Stage::Duplicate;
        self.send_message(KIND_DUPLICATE);
    }

    pub fn reset_after_duplicate(&mut self) {
        if self.stage == Stage::Duplicate {
            self.selected = None;
            self.incoming = None;
            self.progress = 0.0;
            self.error = None;
            self.duplicate_receiver = false;
            self.stage = Stage::Shelf;
        }
    }

    pub fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done_processed)
    }

    fn send_message(&mut self, kind: u8) {
        self.send_payload(kind, &[]);
    }

    fn send_payload(&mut self, kind: u8, payload: &[u8]) {
        if let Some(wire) = self.wire.as_mut() {
            wire.send(0, &message(kind, payload));
        }
    }

    fn send_offer(&mut self, offer: &Offer) -> Result<(), String> {
        let mut payload = Vec::new();
        payload.push(offer.platform.index() as u8);
        let ext = offer.extension.as_bytes();
        payload.push(ext.len().min(255) as u8);
        payload.extend_from_slice(&ext[..ext.len().min(255)]);
        let name = offer.name.as_bytes();
        payload.extend_from_slice(&(name.len().min(u16::MAX as usize) as u16).to_le_bytes());
        payload.extend_from_slice(&name[..name.len().min(u16::MAX as usize)]);
        payload.extend_from_slice(&offer.size.to_le_bytes());
        payload.extend_from_slice(&offer.hash);
        payload.push(offer.assets.len().min(255) as u8);
        for asset in &offer.assets {
            payload.push(asset.kind);
            payload.extend_from_slice(&asset.size.to_le_bytes());
        }
        payload.push(u8::from(offer.shell.is_some()));
        if let Some(shell) = offer.shell {
            payload.extend_from_slice(&shell.colour);
            payload.push(outline_code(shell.outline));
            payload.push(finish_code(shell.finish));
        }
        let wire = self.wire.as_mut().ok_or("对方还没有就绪")?;
        wire.send(0, &message(KIND_OFFER, &payload));
        Ok(())
    }

    fn handle_frame(&mut self, root: &Path, frame: &[u8]) {
        let Some((kind, payload)) = parse(frame) else {
            return;
        };
        match kind {
            KIND_OFFER => {
                if self.stage == Stage::Shelf {
                    if let Some(offer) = parse_offer(payload) {
                        self.peer = Some(offer.name.clone());
                        self.incoming = Some(offer);
                        self.stage = Stage::Incoming;
                    }
                }
            }
            KIND_ACCEPT => {
                if self.stage == Stage::OfferSent {
                    let mask = payload.first().copied().unwrap_or(0);
                    if let Some(selected) = self.selected.as_mut() {
                        if mask != 0 {
                            selected.assets.retain(|asset| {
                                asset.meta.kind != ASSET_ROM
                                    && (mask & (1u8 << asset.meta.kind)) != 0
                            });
                            selected.offer.assets =
                                selected.assets.iter().map(|asset| asset.meta).collect();
                            selected.offer.size =
                                selected.assets.iter().map(|asset| asset.meta.size).sum();
                            if selected.assets.is_empty() {
                                self.error = Some("对方请求的资料为空".into());
                                self.stage = Stage::Failed;
                                return;
                            }
                            self.metadata_only = true;
                        }
                    }
                    self.sending_index = 0;
                    self.sending_at = 0;
                    self.progress = 0.0;
                    self.stage = Stage::Sending;
                }
            }
            KIND_REJECT => {
                if self.stage == Stage::OfferSent {
                    self.stage = Stage::Shelf;
                    self.selected = None;
                    self.progress = 0.0;
                }
            }
            KIND_DATA => {
                if self.stage == Stage::Receiving {
                    self.receive_chunk(payload);
                }
            }
            KIND_DONE => {
                if self.stage == Stage::Receiving {
                    if self.finish_receive(root) {
                        self.send_message(KIND_ACK);
                        self.stage = Stage::Done;
                        self.done_processed = true;
                    }
                } else if self.stage == Stage::Sending {
                    self.stage = Stage::Done;
                    self.done_processed = true;
                }
            }
            KIND_ACK => {
                if self.stage == Stage::Done {
                    self.done_processed = true;
                }
            }
            KIND_DUPLICATE => {
                self.error = Some("对方已拥有当前卡带".into());
                self.duplicate_receiver = false;
                self.stage = Stage::Duplicate;
                self.selected = None;
            }
            KIND_CANCEL => {
                self.error = Some("对方取消了传输".into());
                self.stage = Stage::Failed;
                self.abort_receive();
            }
            _ => {}
        }
    }

    fn receive_chunk(&mut self, payload: &[u8]) {
        if payload.len() < 9 {
            return;
        }
        let kind = payload[0];
        let offset = u64::from_le_bytes(payload[1..9].try_into().unwrap_or([0; 8]));
        let Some(asset) = self.receiving.iter_mut().find(|asset| asset.meta.kind == kind) else {
            return;
        };
        if offset != asset.received {
            self.error = Some("传输数据乱序".into());
            self.stage = Stage::Failed;
            self.abort_receive();
            return;
        }
        if asset
            .file
            .write_all(&payload[9..])
            .is_err()
        {
            self.error = Some("写入卡带文件失败".into());
            self.stage = Stage::Failed;
            self.abort_receive();
            return;
        }
        asset.received += (payload.len() - 9) as u64;
        self.received_total += (payload.len() - 9) as u64;
        self.progress = (self.received_total as f32 / self.receiving_total.max(1) as f32)
            .clamp(0.0, 1.0);
    }

    fn finish_receive(&mut self, root: &Path) -> bool {
        if self.receiving.is_empty() {
            return false;
        }
        if self
            .receiving
            .iter()
            .any(|asset| asset.received != asset.meta.size)
        {
            self.error = Some("传输数据不完整".into());
            self.stage = Stage::Failed;
            self.abort_receive();
            return false;
        }
        for asset in &mut self.receiving {
            if asset.file.flush().is_err() {
                self.error = Some("保存卡带文件失败".into());
                self.stage = Stage::Failed;
                self.abort_receive();
                return false;
            }
        }
        for asset in &self.receiving {
            if std::fs::rename(&asset.temp, &asset.target).is_err() {
                self.error = Some("保存卡带文件失败".into());
                self.stage = Stage::Failed;
                return false;
            }
        }
        if let (Some(stem), Some(offer)) = (&self.target_stem, &self.incoming) {
            if !self.metadata_only {
                if let Some(shell) = offer.shell {
                    let _ = slot_store::ini::write(
                    root,
                    "linkSlot/Config/cart_shell.txt",
                        stem,
                        &shell.to_value(),
                    );
                }
            }
            let rom = self
                .receiving
                .iter()
                .find(|asset| asset.meta.kind == ASSET_ROM)
                .map(|asset| asset.target.clone());
            let label = self
                .receiving
                .iter()
                .find(|asset| asset.meta.kind == ASSET_LABEL)
                .map(|asset| asset.target.clone());
            if let Some(rom) = rom {
                let cart = Cart {
                    platform: offer.platform,
                    stem: stem.clone(),
                    rom,
                    label,
                    title: String::new(),
                    code: String::new(),
                    shell: offer.shell,
                model: Default::default(),
                front: None,
};
                let _ = slot_store::cart_meta::update(root, &cart, |meta| {
                    meta.sha256 = Some(rom_id::digest_hex(&offer.hash));
                });
            }
        }
        self.receiving.clear();
        self.progress = 1.0;
        true
    }

    fn abort_receive(&mut self) {
        for asset in &mut self.receiving {
            let _ = std::fs::remove_file(&asset.temp);
        }
        self.receiving.clear();
    }

    fn pump_send(&mut self, wire: &mut Box<dyn LinkChannel>) {
        let Some(selected) = &mut self.selected else {
            return;
        };
        if self.sending_index >= selected.assets.len() {
            wire.send(0, &message(KIND_DONE, &[]));
            self.stage = Stage::Done;
            self.progress = 1.0;
            self.done_processed = true;
            return;
        }
        let asset = &selected.assets[self.sending_index];
        let mut file = match File::open(&asset.path) {
            Ok(file) => file,
            Err(error) => {
                self.error = Some(format!("读取卡带文件失败：{error}"));
                self.stage = Stage::Failed;
                return;
            }
        };
        if self.sending_at >= asset.meta.size {
            self.sending_index += 1;
            self.sending_at = 0;
            return;
        }
        if file.seek(std::io::SeekFrom::Start(self.sending_at)).is_err() {
            self.error = Some("读取卡带文件失败".into());
            self.stage = Stage::Failed;
            return;
        }
        let remaining = (asset.meta.size - self.sending_at).min(CHUNK as u64) as usize;
        let mut buf = vec![0u8; remaining];
        match file.read_exact(&mut buf) {
            Ok(()) => {
                let mut payload = Vec::with_capacity(9 + buf.len());
                payload.push(asset.meta.kind);
                payload.extend_from_slice(&self.sending_at.to_le_bytes());
                payload.extend_from_slice(&buf);
                wire.send(0, &message(KIND_DATA, &payload));
                self.sending_at += buf.len() as u64;
                self.progress = if selected.offer.size == 0 {
                    1.0
                } else {
                    let done: u64 = selected.assets[..self.sending_index]
                        .iter()
                        .map(|asset| asset.meta.size)
                        .sum::<u64>()
                        + self.sending_at;
                    (done as f32 / selected.offer.size as f32).clamp(0.0, 1.0)
                };
                if self.sending_at >= asset.meta.size {
                    self.sending_index += 1;
                    self.sending_at = 0;
                }
            }
            Err(error) => {
                self.error = Some(format!("读取卡带文件失败：{error}"));
                self.stage = Stage::Failed;
            }
        }
    }
}

pub fn asset_label(kind: u8) -> &'static str {
    match kind {
        ASSET_ROM => "ROM",
        ASSET_LABEL => "封面",
        ASSET_INTRO => "介绍",
        ASSET_VIDEO => "视频",
        _ => "资料",
    }
}

fn message(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(MAGIC);
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

fn parse(frame: &[u8]) -> Option<(u8, &[u8])> {
    (frame.len() >= 5 && &frame[..4] == MAGIC).then(|| (frame[4], &frame[5..]))
}

fn parse_offer(payload: &[u8]) -> Option<Offer> {
    let first = *payload.first()?;
    let platform = [
        Platform::Gba,
        Platform::Gb,
        Platform::Gbc,
        Platform::Fc,
        Platform::Sfc,
    ]
    .into_iter()
    .find(|p| p.index() as u8 == first)?;
    let mut at = 1;
    let ext_len = *payload.get(at)? as usize;
    at += 1;
    let extension = std::str::from_utf8(payload.get(at..at + ext_len)?)
        .ok()?
        .to_string();
    at += ext_len;
    let name_len = u16::from_le_bytes(payload.get(at..at + 2)?.try_into().ok()?) as usize;
    at += 2;
    let name = String::from_utf8_lossy(payload.get(at..at + name_len)?).into_owned();
    at += name_len;
    let size = u64::from_le_bytes(payload.get(at..at + 8)?.try_into().ok()?);
    at += 8;
    let hash: Digest = payload.get(at..at + 32)?.try_into().ok()?;
    at += 32;
    let count = *payload.get(at)? as usize;
    at += 1;
    let mut assets = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = *payload.get(at)?;
        at += 1;
        let size = u64::from_le_bytes(payload.get(at..at + 8)?.try_into().ok()?);
        at += 8;
        assets.push(AssetMeta { kind, size });
    }
    let has_shell = *payload.get(at)? != 0;
    at += 1;
    let shell = if has_shell {
        let colour = payload.get(at..at + 3)?.try_into().ok()?;
        at += 3;
        let outline = outline_from(*payload.get(at)?);
        at += 1;
        let finish = finish_from(*payload.get(at)?);
        Some(ShellChoice {
            outline,
            colour,
            finish,
        })
    } else {
        None
    };
    Some(Offer {
        platform,
        extension,
        name,
        size,
        hash,
        assets,
        shell,
    })
}

fn outline_code(outline: Outline) -> u8 {
    match outline {
        Outline::Auto => 0,
        Outline::Notched => 1,
        Outline::Rounded => 2,
    }
}

fn outline_from(code: u8) -> Outline {
    match code {
        1 => Outline::Notched,
        2 => Outline::Rounded,
        _ => Outline::Auto,
    }
}

fn finish_code(finish: ShellFinish) -> u8 {
    match finish {
        ShellFinish::Solid => 0,
        ShellFinish::Clear => 1,
        ShellFinish::Glitter => 2,
    }
}

fn finish_from(code: u8) -> ShellFinish {
    match code {
        1 => ShellFinish::Clear,
        2 => ShellFinish::Glitter,
        _ => ShellFinish::Solid,
    }
}

fn source_assets(root: &Path, cart: &Cart) -> Result<Vec<SourceAsset>, String> {
    let mut assets = Vec::new();
    let size = std::fs::metadata(&cart.rom)
        .map_err(|e| format!("读取卡带失败：{e}"))?
        .len();
    assets.push(SourceAsset {
        meta: AssetMeta {
            kind: ASSET_ROM,
            size,
        },
        path: cart.rom.clone(),
    });
    if let Some(label) = &cart.label {
        if let Ok(meta) = std::fs::metadata(label) {
            assets.push(SourceAsset {
                meta: AssetMeta {
                    kind: ASSET_LABEL,
                    size: meta.len(),
                },
                path: label.clone(),
            });
        }
    }
    let info = root.join("Info").join(cart.platform.dir_name());
    for (kind, extension) in [(ASSET_INTRO, "txt"), (ASSET_VIDEO, "mp4")] {
        let path = info.join(format!("{}.{}", cart.stem, extension));
        if let Ok(meta) = std::fs::metadata(&path) {
            assets.push(SourceAsset {
                meta: AssetMeta {
                    kind,
                    size: meta.len(),
                },
                path,
            });
        }
    }
    Ok(assets)
}

fn unique_stem(root: &Path, platform: Platform, stem: &str, extension: &str) -> String {
    let dir = root.join("Games").join(platform.dir_name());
    for index in 1..=9999u32 {
        let candidate = if index == 1 {
            stem.to_string()
        } else {
            format!("{stem} ({index})")
        };
        let rom = dir.join(format!("{candidate}.{extension}"));
        if !rom.exists() {
            return candidate;
        }
    }
    format!("{stem} (copy)")
}

fn asset_paths(
    root: &Path,
    platform: Platform,
    stem: &str,
    extension: &str,
    kind: u8,
) -> Result<(PathBuf, PathBuf), String> {
    let (dir, ext) = match kind {
        ASSET_ROM => (root.join("Games").join(platform.dir_name()), extension.to_string()),
        ASSET_LABEL => (root.join("Labels").join(platform.dir_name()), "png".to_string()),
        ASSET_INTRO => (root.join("Info").join(platform.dir_name()), "txt".to_string()),
        ASSET_VIDEO => (root.join("Info").join(platform.dir_name()), "mp4".to_string()),
        _ => return Err("对方发来了未知的卡带内容".into()),
    };
    let target = dir.join(format!("{stem}.{ext}"));
    let temp = dir.join(format!(".{stem}.{ext}.part"));
    let _ = std::fs::remove_file(&temp);
    Ok((temp, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_net::TcpLink;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    fn attached(wire: TcpLink) -> Transfer {
        Transfer {
            host: true,
            stage: Stage::Shelf,
            starter: None,
            wire: Some(Box::new(wire)),
            selected: None,
            incoming: None,
            progress: 0.0,
            peer: None,
            received: false,
            duplicate_receiver: false,
            metadata_missing: Vec::new(),
            metadata_only: false,
            metadata_stem: None,
            error: None,
            sending_index: 0,
            sending_at: 0,
            receiving: Vec::new(),
            receiving_total: 0,
            received_total: 0,
            target_stem: None,
            done_processed: false,
        }
    }

    fn pair() -> (Transfer, Transfer) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let host = std::thread::spawn(move || TcpLink::host("127.0.0.1", port));
        std::thread::sleep(Duration::from_millis(80));
        let client = TcpLink::join("127.0.0.1", port).unwrap();
        let host = host.join().unwrap().unwrap();
        (attached(host), attached(client))
    }

    fn source_cart(root: &Path) -> Cart {
        let dir = root.join("Games/GBA");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Source.gba"), vec![0x41; 150_000]).unwrap();
        std::fs::create_dir_all(root.join("Labels/GBA")).unwrap();
        std::fs::create_dir_all(root.join("Info/GBA")).unwrap();
        std::fs::write(root.join("Labels/GBA/Source.png"), b"cover").unwrap();
        std::fs::write(root.join("Info/GBA/Source.txt"), b"intro").unwrap();
        Cart {
            platform: Platform::Gba,
            stem: "Source".into(),
            rom: dir.join("Source.gba"),
            label: Some(root.join("Labels/GBA/Source.png")),
            title: String::new(),
            code: String::new(),
            shell: Some(ShellChoice {
                outline: Outline::Auto,
                colour: [0x12, 0x34, 0x56],
                finish: ShellFinish::Solid,
            }),
        model: Default::default(),
        front: None,
}
    }

    #[test]
    fn an_automatic_shell_colour_is_carried_by_the_offer() {
        let root = tempfile::tempdir().unwrap();
        let mut cart = source_cart(root.path());
        cart.shell = None;
        cart.code = "AXV".into();
        let (mut sender, _receiver) = pair();
        sender.select(root.path(), &cart, [0u8; 32]).unwrap();
        let shell = sender
            .selected
            .as_ref()
            .expect("selected offer")
            .offer
            .shell
            .expect("effective shell");
        assert_eq!(shell.colour, [0xc2, 0x33, 0x2e]);
    }

    #[test]
    fn a_cartridge_moves_its_rom_cover_and_intro_but_not_its_save() {
        let sender_root = tempfile::tempdir().unwrap();
        let receiver_root = tempfile::tempdir().unwrap();
        let cart = source_cart(sender_root.path());
        std::fs::create_dir_all(sender_root.path().join("Saves/GBA")).unwrap();
        std::fs::write(sender_root.path().join("Saves/GBA/Source.sav"), b"save").unwrap();
        let (mut sender, mut receiver) = pair();

        sender.select(sender_root.path(), &cart, rom_id::digest_file(&cart.rom).unwrap_or([0u8; 32])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while receiver.stage != Stage::Incoming && Instant::now() < deadline {
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(receiver.stage, Stage::Incoming);
        let incoming = receiver.incoming.as_ref().expect("incoming offer");
        assert_eq!(incoming.name, "Source");
        assert_eq!(
            incoming.shell.map(|shell| shell.colour),
            Some([0x12, 0x34, 0x56])
        );
        receiver.accept(receiver_root.path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while (sender.stage != Stage::Done || receiver.stage != Stage::Done)
            && Instant::now() < deadline
        {
            sender.update(sender_root.path());
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(sender.stage, Stage::Done, "{:?}", sender.error);
        assert_eq!(receiver.stage, Stage::Done, "{:?}", receiver.error);
        assert_eq!(
            std::fs::read(receiver_root.path().join("Games/GBA/Source.gba")).unwrap(),
            vec![0x41; 150_000]
        );
        assert!(receiver_root.path().join("Labels/GBA/Source.png").is_file());
        assert!(receiver_root.path().join("Info/GBA/Source.txt").is_file());
        assert!(!receiver_root.path().join("Saves/GBA/Source.sav").exists());
        assert!(receiver_root.path().join("linkSlot/Config/cart_shell.txt").is_file());
    }

    #[test]
    fn an_offer_can_be_rejected_without_writing_a_file() {
        let sender_root = tempfile::tempdir().unwrap();
        let receiver_root = tempfile::tempdir().unwrap();
        let cart = source_cart(sender_root.path());
        let (mut sender, mut receiver) = pair();
        sender.select(sender_root.path(), &cart, rom_id::digest_file(&cart.rom).unwrap_or([0u8; 32])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while receiver.stage != Stage::Incoming && Instant::now() < deadline {
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        receiver.reject();
        let deadline = Instant::now() + Duration::from_secs(5);
        while sender.stage != Stage::Shelf && Instant::now() < deadline {
            sender.update(sender_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!receiver_root.path().join("Games/GBA/Source.gba").exists());
    }

    #[test]
    fn a_duplicate_rom_can_receive_only_its_missing_side_assets() {
        let sender_root = tempfile::tempdir().unwrap();
        let receiver_root = tempfile::tempdir().unwrap();
        let cart = source_cart(sender_root.path());
        std::fs::write(
            sender_root.path().join("Info/GBA/Source.mp4"),
            b"video",
        )
        .unwrap();

        let receiver_labels = receiver_root.path().join("Labels/GBA");
        let receiver_info = receiver_root.path().join("Info/GBA");
        std::fs::create_dir_all(&receiver_labels).unwrap();
        std::fs::create_dir_all(&receiver_info).unwrap();
        std::fs::create_dir_all(receiver_root.path().join("Games/GBA")).unwrap();
        std::fs::write(
            receiver_root.path().join("Games/GBA/Source.gba"),
            vec![0x41; 150_000],
        )
        .unwrap();
        std::fs::write(receiver_info.join("Source.txt"), b"local intro").unwrap();
        let receiver_cart = Cart {
            platform: Platform::Gba,
            stem: "Source".into(),
            rom: receiver_root.path().join("Games/GBA/Source.gba"),
            label: None,
            title: String::new(),
            code: String::new(),
            shell: Some(ShellChoice {
                outline: Outline::Auto,
                colour: [1, 2, 3],
                finish: ShellFinish::Solid,
            }),
        model: Default::default(),
        front: None,
};

        let (mut sender, mut receiver) = pair();
        sender
            .select(sender_root.path(), &cart, rom_id::digest_file(&cart.rom).unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while receiver.stage != Stage::Incoming && Instant::now() < deadline {
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(receiver.stage, Stage::Incoming);
        let offer = receiver.incoming.as_ref().unwrap();
        let missing = offer.metadata_missing(receiver_root.path(), &receiver_cart);
        assert_eq!(missing, vec![ASSET_LABEL, ASSET_VIDEO]);
        receiver.offer_metadata("Source".into(), missing);
        assert_eq!(receiver.stage, Stage::IncomingMeta);
        assert_eq!(receiver.display_assets(), vec!["封面", "视频"]);
        receiver.accept_metadata(receiver_root.path()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while (sender.stage != Stage::Done || receiver.stage != Stage::Done)
            && Instant::now() < deadline
        {
            sender.update(sender_root.path());
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(sender.stage, Stage::Done, "{:?}", sender.error);
        assert_eq!(receiver.stage, Stage::Done, "{:?}", receiver.error);
        assert_eq!(
            std::fs::read(receiver_info.join("Source.txt")).unwrap(),
            b"local intro"
        );
        assert_eq!(
            std::fs::read(receiver_labels.join("Source.png")).unwrap(),
            b"cover"
        );
        assert_eq!(
            std::fs::read(receiver_info.join("Source.mp4")).unwrap(),
            b"video"
        );
        assert!(!receiver_root
            .path()
            .join("Games/GBA/Source (2).gba")
            .exists());
        assert!(!receiver_root
            .path()
            .join("linkSlot/Config/cart_shell.txt")
            .exists());
    }

    #[test]
    fn a_duplicate_frame_is_read_even_when_the_receiver_closes() {
        let sender_root = tempfile::tempdir().unwrap();
        let receiver_root = tempfile::tempdir().unwrap();
        let cart = source_cart(sender_root.path());
        let (mut sender, mut receiver) = pair();
        sender
            .select(sender_root.path(), &cart, rom_id::digest_file(&cart.rom).unwrap_or([0u8; 32]))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while receiver.stage != Stage::Incoming && Instant::now() < deadline {
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        receiver.mark_duplicate(true);
        if let Some(wire) = receiver.wire.as_mut() {
            wire.send_end();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while sender.stage != Stage::Duplicate && Instant::now() < deadline {
            sender.update(sender_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(sender.stage, Stage::Duplicate, "{:?}", sender.error);
    }

    #[test]
    fn a_receiver_that_already_has_the_hash_refuses_without_writing() {
        let sender_root = tempfile::tempdir().unwrap();
        let receiver_root = tempfile::tempdir().unwrap();
        let cart = source_cart(sender_root.path());
        // Same bytes, same platform and therefore the same ROM hash.
        source_cart(receiver_root.path());
        let (mut sender, mut receiver) = pair();
        sender.select(sender_root.path(), &cart, rom_id::digest_file(&cart.rom).unwrap_or([0u8; 32])).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while receiver.stage != Stage::Incoming && Instant::now() < deadline {
            receiver.update(receiver_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        receiver.mark_duplicate(true);
        assert_eq!(receiver.stage, Stage::Duplicate);
        assert!(receiver.duplicate_receiver);
        let deadline = Instant::now() + Duration::from_secs(5);
        while sender.stage != Stage::Duplicate && Instant::now() < deadline {
            sender.update(sender_root.path());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(sender.stage, Stage::Duplicate);
        assert!(!sender.duplicate_receiver);
        assert!(!receiver_root.path().join("Games/GBA/Source (2).gba").exists());
        assert_eq!(
            std::fs::read(receiver_root.path().join("Games/GBA/Source.gba")).unwrap(),
            vec![0x41; 150_000]
        );
    }
}
