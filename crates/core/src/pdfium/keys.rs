//! Códigos de tecla (estilo Windows VK, que es lo que espera PDFium) y
//! modificadores FWL_EVENTFLAG.

use super::sys;

pub const MOD_SHIFT: i32 = sys::FWL_EVENTFLAG_ShiftKey as i32;
pub const MOD_CTRL: i32 = sys::FWL_EVENTFLAG_ControlKey as i32;
pub const MOD_ALT: i32 = sys::FWL_EVENTFLAG_AltKey as i32;
pub const MOD_META: i32 = sys::FWL_EVENTFLAG_MetaKey as i32;
pub const MOD_LBUTTON: i32 = sys::FWL_EVENTFLAG_LeftButtonDown as i32;

pub const VK_BACK: i32 = sys::FWL_VKEY_Back as i32;
pub const VK_TAB: i32 = sys::FWL_VKEY_Tab as i32;
pub const VK_RETURN: i32 = sys::FWL_VKEY_Return as i32;
pub const VK_ESCAPE: i32 = sys::FWL_VKEY_Escape as i32;
pub const VK_SPACE: i32 = sys::FWL_VKEY_Space as i32;
pub const VK_PRIOR: i32 = sys::FWL_VKEY_Prior as i32;
pub const VK_NEXT: i32 = sys::FWL_VKEY_Next as i32;
pub const VK_END: i32 = sys::FWL_VKEY_End as i32;
pub const VK_HOME: i32 = sys::FWL_VKEY_Home as i32;
pub const VK_LEFT: i32 = sys::FWL_VKEY_Left as i32;
pub const VK_UP: i32 = sys::FWL_VKEY_Up as i32;
pub const VK_RIGHT: i32 = sys::FWL_VKEY_Right as i32;
pub const VK_DOWN: i32 = sys::FWL_VKEY_Down as i32;
pub const VK_INSERT: i32 = sys::FWL_VKEY_Insert as i32;
pub const VK_DELETE: i32 = sys::FWL_VKEY_Delete as i32;
pub const VK_A: i32 = sys::FWL_VKEY_A as i32;
pub const VK_0: i32 = sys::FWL_VKEY_0 as i32;
pub const VK_F1: i32 = sys::FWL_VKEY_F1 as i32;

/// Teclas que en Windows generan además un WM_CHAR. PDFium (como Chrome)
/// espera recibir ambos eventos.
pub fn char_for_vk(vk: i32) -> Option<i32> {
    match vk {
        VK_BACK => Some(8),
        VK_TAB => Some(9),
        VK_RETURN => Some(13),
        VK_ESCAPE => Some(27),
        _ => None,
    }
}
