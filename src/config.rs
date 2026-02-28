use anyhow::Result;

/// Application configuration
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENROUTER_API_KEY environment variable not set"))?;

        let base_url = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());

        let model = std::env::var("OPENROUTER_MODEL")
            .unwrap_or_else(|_| "mistralai/codestral-2508".to_string());

        Ok(Self {
            api_key,
            base_url,
            model,
        })
    }
}

/// Initialize the logger
pub fn _init_logger() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Debug)
        .init();
}
