//! USB 2.0 descriptor and EHCI register layout definitions.
//!
//! Derived from OSyx GPL-3.0 USB library, commit
//! 1b1665e9add219c9ea1f38fa9bbbf7290d112978.
//! https://github.com/OSyxSoftware/OSyx (see repository LICENSE).
//! These data structures alone do not run a USB host controller.

pub mod descriptor;
pub mod ehci;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed { Full, Low, High, Super, SuperPlus }
