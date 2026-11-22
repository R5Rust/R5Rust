//! Weapon slots, Apex style: slot 1 = the gun, the melee slot = empty hands or the equipped heirloom
//! (weaponSelectPrimary0 / weaponSelectPrimary2 in retail's binds). A switch lowers the current item (its holster
//! sequence for holster_time), then draws the other (drawfirst the first time, deploy_time after). The melee key
//! with the gun out is a quick melee: the melee item swings at once and the gun comes back up afterwards.
//! Movement follows the item in hand (retail move_speed_modifier; 1 for every gun and melee item, so running with a
//! gun out is as fast as holstered, like the real game), ADS slows by the gun's ads_move_speed_scale.

use crate::melee::{Melee, MeleeInput};
use crate::viewmodel::Viewmodel;
use crate::weapon::{Weapon, WeaponInput};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Gun,
    Melee,
}

pub struct Loadout {
    pub slot: Slot,
    /// switching to this slot (the current item is lowering)
    pub pending: Option<Slot>,
    /// quick melee from the gun in progress
    pub quick: bool,
}

impl Default for Loadout {
    fn default() -> Self {
        Loadout { slot: Slot::Gun, pending: None, quick: false }
    }
}

/// This tick's buttons for the hands.
#[derive(Default, Clone, Copy)]
pub struct HandsInput {
    pub slot_gun: bool,
    pub slot_melee: bool,
    pub melee: bool,
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    pub inspect: bool,
}

impl Loadout {
    /// Is the melee item the one on screen (its viewmodel drawn)?
    pub fn melee_shown(&self) -> bool {
        self.quick || self.slot == Slot::Melee
    }

    pub fn request(&mut self, to: Slot, gun: Option<&mut Weapon>, melee: Option<&mut Melee>, melee_vm: Option<&Viewmodel>) {
        if self.pending.is_some() || self.quick || to == self.slot {
            return;
        }
        self.pending = Some(to);
        match self.slot {
            Slot::Gun => {
                if let Some(w) = gun {
                    w.start_holster();
                }
            }
            Slot::Melee => {
                if let (Some(m), Some(vm)) = (melee, melee_vm) {
                    m.start_holster(vm);
                }
            }
        }
    }

    /// Switching and quick melee; returns the gun's input for this tick (empty while it isn't in hand).
    pub fn tick(&mut self, hi: HandsInput, gun: &mut Option<Weapon>, melee: &mut Option<Melee>, melee_vm: Option<&Viewmodel>) -> (WeaponInput, MeleeInput) {
        if hi.slot_gun {
            self.request(Slot::Gun, gun.as_mut(), melee.as_mut(), melee_vm);
        }
        if hi.slot_melee {
            self.request(Slot::Melee, gun.as_mut(), melee.as_mut(), melee_vm);
        }
        // quick melee from the gun: the gun drops out of view at once, the melee item swings
        if hi.melee && self.slot == Slot::Gun && self.pending.is_none() && !self.quick {
            if let (Some(m), Some(vm)) = (melee.as_mut(), melee_vm) {
                if let Some(w) = gun.as_mut() {
                    w.start_holster();
                }
                m.swing(vm);
                self.quick = true;
            }
        }
        if self.quick && melee.as_ref().is_none_or(|m| !m.attacking()) {
            self.quick = false;
            if let Some(w) = gun.as_mut() {
                w.deploy();
            }
        }
        // the lowering item is down: bring up the other
        if let Some(to) = self.pending {
            let down = match self.slot {
                Slot::Gun => gun.as_ref().is_none_or(|w| w.holstered()),
                Slot::Melee => melee.as_ref().is_none_or(|m| m.holstered()),
            };
            if down {
                self.slot = to;
                self.pending = None;
                match to {
                    Slot::Gun => {
                        if let Some(w) = gun.as_mut() {
                            w.deploy();
                        }
                    }
                    Slot::Melee => {
                        if let (Some(m), Some(vm)) = (melee.as_mut(), melee_vm) {
                            m.deploy(vm);
                        }
                    }
                }
            }
        }
        let gun_live = self.slot == Slot::Gun && self.pending.is_none() && !self.quick;
        let melee_live = self.slot == Slot::Melee && self.pending.is_none();
        let wi = if gun_live { WeaponInput { fire: hi.fire, ads: hi.ads, reload: hi.reload, inspect: hi.inspect, ..Default::default() } } else { WeaponInput::default() };
        let mi = MeleeInput { attack: melee_live && (hi.fire || hi.melee), inspect: melee_live && hi.inspect, ..Default::default() };
        (wi, mi)
    }

    /// Movement scale for the item in hand.
    pub fn speed_scale(&self, gun: Option<&Weapon>, melee: Option<&Melee>) -> f32 {
        if self.melee_shown() {
            melee.map_or(1.0, |m| m.stats.move_speed)
        } else {
            gun.map_or(1.0, |w| w.stats.move_speed * if w.ads > 0.5 { w.stats.ads_move } else { 1.0 })
        }
    }
}
