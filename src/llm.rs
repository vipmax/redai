use async_openai::{config::OpenAIConfig, Client};
use serde_json::{json, Value};

pub struct LlmClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl LlmClient {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url);
        
        let client = Client::with_config(config);

        Self {
            client,
            model: model.into(),
        }
    }

    pub async fn chat(&self, messages: Vec<Value>) -> anyhow::Result<String> {
        let request = json!({ "model": self.model, "messages": messages });
        let response: Value = self.client.chat().create_byot(request).await?;
        let content = response["choices"][0]["message"]["content"]
            .as_str().unwrap_or("").to_string();

        Ok(content)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use dotenv::dotenv;
    use crate::prompts::{SYSTEM_PROMPT, REMINDER};

    #[tokio::test]
    #[ignore]
    async fn test_openrouter_chat() -> anyhow::Result<()> {
        dotenv()?;

        let api_key = std::env::var("OPENROUTER_API_KEY")?;
        let base_url = "https://openrouter.ai/api/v1";
        let model = "mistralai/codestral-2501";

        let client = LlmClient::new(&api_key, base_url, model);
        
        let code = indoc! {r#"
            fn main() {
                for i in 0..5 {
                    println!("value: {}", <|cursor|>);
                }
            }
        "#};
        
        println!("code:\n{}", code);
        
        let messages = vec![
            json!({ "role": "system", "content": SYSTEM_PROMPT }),
            json!({ "role": "user", "content": format!("small context:\n{}", code) }),
            json!({ "role": "user", "content": REMINDER }),
        ];

        let reply = client.chat(messages).await?;
        println!("llm response:\n{}", reply);
        
        // assert!(reply.contains(
        //     r#"<|SEARCH|>println!("value: {}", <|cursor|>);<|DIVIDE|>println!("value: {}", i);<|REPLACE|>"#
        // ));

        Ok(())
    }
}