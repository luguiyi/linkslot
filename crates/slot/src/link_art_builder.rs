use std::sync::mpsc::{channel, Receiver};

use slot_i18n::Language;
use slot_ui::{link_art, LinkArt};

pub struct LinkArtBuilder {
    built: Receiver<LinkArt>,
}

impl LinkArtBuilder {
    pub fn spawn(lang: Language) -> Self {
        let (tx, built) = channel();
        let spawned = std::thread::Builder::new()
            .name("slot-link-art".into())
            .spawn(move || {
                let _ = tx.send(link_art(lang));
            });
        if let Err(e) = spawned {
            eprintln!("slot: link art worker: {e}");
        }
        LinkArtBuilder { built }
    }

    pub fn take(&self) -> Option<LinkArt> {
        self.built.try_recv().ok()
    }
}
