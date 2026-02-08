pub mod autologin;
pub mod selenium;

pub use autologin::{maybe_autologin, maybe_autologin_for_os, AutoLoginOptions};
pub use selenium::{Element, WebDriver};
