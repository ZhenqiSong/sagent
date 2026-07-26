
use crate::cli_core::skin::get_active_skin;

pub fn build_compact_banner() -> String{
    let skin = get_active_skin();
    let skin_name = skin.name.clone();
}