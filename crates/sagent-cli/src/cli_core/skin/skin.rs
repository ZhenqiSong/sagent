use std::sync::{Arc, RwLock};
use once_cell::sync::Lazy;
use super::config::SkinConfig;


struct ActiveSkin{
    name: String,
    skin: Option<Arc<SkinConfig>>
}

static ACTIVE_SKIN: Lazy<RwLock<ActiveSkin>> = Lazy::new(|| {
    RwLock::new(ActiveSkin { name: "default".to_string(), skin: None })
});

pub fn get_active_skin() -> SkinConfig {
    if let Some(skin) = ACTIVE_SKIN.read()
            .expect("skin lock poisoned").skin.as_ref() {
        return (**skin).clone();
    }

    let mut g = ACTIVE_SKIN.write().expect("skin lock poisoned");
    if let Some(skin) = g.skin.as_ref() {
        return (**skin).clone();
    }
    let skin = Arc::new(SkinConfig::load(&g.name));
    let config = (*skin).clone();
    g.skin = Some(skin);
    config
}