use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use sagent_common::get_sagent_home;

#[derive(Clone, Serialize, Deserialize)]
pub struct SkinConfig{
    #[serde(default = "unknown_name")]
    pub name: String,
    #[serde(default = "empty_description")]
    pub description: String,
    #[serde(default)]
    pub colors: HashMap<String, String>,
}

fn unknown_name() -> String {
    "unknown".to_string()
}

fn empty_description() -> String {
    "".to_string()
}

impl Default for SkinConfig{
    fn default()->SkinConfig{
        SkinConfig{
            name: "default".to_string(),
            description: "".to_string(),
            colors: HashMap::new(),
        }
    }
}

impl SkinConfig{

    fn skin_dir() -> PathBuf{
        get_sagent_home().join("skins")
    }
    pub fn load(name: &str) -> SkinConfig {
        let skin_file = Self::skin_dir().join(name);
        if skin_file.is_file() {
            let file = fs::File::open(&skin_file).unwrap();
            serde_json::from_reader(file).unwrap()
        } else {
            SkinConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_skin_config() {
        let config = SkinConfig::default();
        assert_eq!(config.name, "default");
        assert_eq!(config.description, "");
    }

    #[test]
    fn test_deserialize_full_config() {
        let json = r#"{"name": "my-skin", "description": "A custom skin"}"#;
        let config: SkinConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "my-skin");
        assert_eq!(config.description, "A custom skin");
    }

    #[test]
    fn test_deserialize_missing_fields() {
        let json = r#"{}"#;
        let config: SkinConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "unknown");
        assert_eq!(config.description, "");
    }

    #[test]
    fn test_deserialize_missing_name_only() {
        let json = r#"{"description": "some desc"}"#;
        let config: SkinConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "unknown");
        assert_eq!(config.description, "some desc");
    }

    #[test]
    fn test_deserialize_missing_description_only() {
        let json = r#"{"name": "test-skin"}"#;
        let config: SkinConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "test-skin");
        assert_eq!(config.description, "");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let config = SkinConfig {
            name: "roundtrip".to_string(),
            description: "test".to_string(),
            colors: HashMap::new(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SkinConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, config.name);
        assert_eq!(parsed.description, config.description);
    }
}

