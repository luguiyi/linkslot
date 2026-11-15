use slot_input::{Btn, RawEvent};
use slot_store::{ButtonLayout, Controls, Device, GameButton};

/// Remember the binding selected on press, so a menu or setting change cannot
/// strand a held shoulder. Multiple physical keys may share a logical button.
#[derive(Default)]
pub struct Remap {
    held: Vec<(Btn, Btn)>,
}

fn device_btn(device: Device) -> Btn {
    match device {
        Device::Up => Btn::Up,
        Device::Down => Btn::Down,
        Device::Left => Btn::Left,
        Device::Right => Btn::Right,
        Device::A => Btn::A,
        Device::B => Btn::B,
        Device::X => Btn::X,
        Device::Y => Btn::Y,
        Device::L1 => Btn::L1,
        Device::R1 => Btn::R1,
        Device::L2 => Btn::L2,
        Device::R2 => Btn::R2,
        Device::Select => Btn::Select,
        Device::Start => Btn::Start,
    }
}

fn game_btn(button: GameButton) -> Btn {
    match button {
        GameButton::Up => Btn::Up,
        GameButton::Down => Btn::Down,
        GameButton::Left => Btn::Left,
        GameButton::Right => Btn::Right,
        GameButton::A => Btn::A,
        GameButton::B => Btn::B,
        GameButton::Select => Btn::Select,
        GameButton::Start => Btn::Start,
        GameButton::L => Btn::L1,
        GameButton::R => Btn::R1,
        GameButton::X => Btn::X,
        GameButton::Y => Btn::Y,
        GameButton::TurboA => Btn::X,
        GameButton::TurboB => Btn::Y,
    }
}

/// Translate a physical key through the user's Controls file. Defaults are
/// identity, so an absent/old config keeps the old button behaviour.
fn controls_map(controls: &Controls, physical: Btn) -> Option<Btn> {
    GameButton::ALL.into_iter().find_map(|button| {
        let binding = controls.game(button)?;
        (binding.modifier.is_none() && device_btn(binding.button) == physical)
            .then(|| game_btn(button))
    })
}

impl Remap {
    pub fn feed(&mut self, event: RawEvent, layout: ButtonLayout) -> Option<RawEvent> {
        self.feed_with(event, layout, &Controls::default())
    }

    pub fn feed_with(
        &mut self,
        event: RawEvent,
        layout: ButtonLayout,
        controls: &Controls,
    ) -> Option<RawEvent> {
        let (RawEvent::Down(key) | RawEvent::Up(key)) = event;
        let custom = if controls == &Controls::default() {
            None
        } else {
            controls_map(controls, key)
        };
        if custom.is_none() && !matches!(key, Btn::X | Btn::Y | Btn::L1 | Btn::R1 | Btn::L2 | Btn::R2)
        {
            return Some(event);
        }
        match event {
            RawEvent::Down(physical) => {
                if self.held.iter().any(|(p, _)| *p == physical) {
                    return None;
                }
                let logical = custom.unwrap_or(match (layout, physical) {
                    (ButtonLayout::SwapShoulders | ButtonLayout::Both, Btn::L1) => Btn::L2,
                    (ButtonLayout::SwapShoulders | ButtonLayout::Both, Btn::R1) => Btn::R2,
                    (ButtonLayout::SwapShoulders | ButtonLayout::Both, Btn::L2) => Btn::L1,
                    (ButtonLayout::SwapShoulders | ButtonLayout::Both, Btn::R2) => Btn::R1,
                    (ButtonLayout::XyShoulders | ButtonLayout::Both, Btn::X) => Btn::L1,
                    (ButtonLayout::XyShoulders | ButtonLayout::Both, Btn::Y) => Btn::R1,
                    _ => physical,
                });
                let already = self.held.iter().any(|(_, l)| *l == logical);
                self.held.push((physical, logical));
                (!already).then_some(RawEvent::Down(logical))
            }
            RawEvent::Up(physical) => {
                let i = self.held.iter().position(|(p, _)| *p == physical)?;
                let (_, logical) = self.held.remove(i);
                (!self.held.iter().any(|(_, l)| *l == logical)).then_some(RawEvent::Up(logical))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slot_store::Binding;

    #[test]
    fn a_custom_control_moves_the_physical_button() {
        let mut controls = Controls::default();
        controls.set_game(GameButton::A, Binding::plain(Device::B));
        let mut remap = Remap::default();
        assert_eq!(
            remap.feed_with(
                RawEvent::Down(Btn::B),
                ButtonLayout::Off,
                &controls
            ),
            Some(RawEvent::Down(Btn::A))
        );
        assert_eq!(
            remap.feed_with(RawEvent::Up(Btn::B), ButtonLayout::Off, &controls),
            Some(RawEvent::Up(Btn::A))
        );
    }

    #[test]
    fn the_default_controls_keep_the_old_layout() {
        let mut remap = Remap::default();
        assert_eq!(
            remap.feed(RawEvent::Down(Btn::L1), ButtonLayout::SwapShoulders),
            Some(RawEvent::Down(Btn::L2))
        );
    }
}
