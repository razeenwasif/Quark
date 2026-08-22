//! Quark's design system and shared widgets.
//!
//! Split out from the application so the palette can be contrast-tested without
//! a window, and so the theme is defined in exactly one place.

pub mod icons;
pub mod theme;
pub mod widgets;

pub use icons::Icon;
pub use theme::{Palette, ThemeMode};
