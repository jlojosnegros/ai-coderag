use std::collections::HashMap;
use std::env;

pub struct Config {
    pub max_items: usize,
    pub output_path: String,
    pub verbose: bool,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            max_items: env::var("MAX_ITEMS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            output_path: env::var("OUTPUT_PATH")
                .unwrap_or_else(|_| "./output".to_string()),
            verbose: env::var("VERBOSE")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false),
        }
    }

    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Config {
            max_items: map
                .get("max_items")
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            output_path: map
                .get("output_path")
                .cloned()
                .unwrap_or_else(|| "./output".to_string()),
            verbose: map.get("verbose").map(|v| v == "true").unwrap_or(false),
        }
    }
}