#![no_std]

use defmt::Format;

pub mod input_touch;
pub mod rmt_rgb_strip;

#[derive(Clone, Copy, PartialEq, Eq, Format)]
pub enum InputEvent {
    FlTouch,
    FrTouch,
    RlTouch,
    RrTouch,
    ProxNear,
    ProxTouch,
}
