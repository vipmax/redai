use std::{io::stdout};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};
use std::env;
use std::fs;
use dotenv::dotenv;
use ratatui_code_editor::utils::get_lang;

mod utils;
mod diff;
mod llm;
mod prompts;
mod coder;
mod config;
mod tracker;
mod watcher;
mod app;

use app::App;
use config::Config;
use llm::LlmClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    set_panic_hook();

    let config = Config::from_env().unwrap();
    let Config { api_key, base_url, model } = config;

    let args: Vec<String> = env::args().collect();

    let filename = if args.len() > 1 {
        args[1].clone()
    } else {
       "".to_string()
    };

    let (language, content) = if filename.is_empty() {
        (String::new(), String::new())
    } else {
        (get_lang(&filename), fs::read_to_string(&filename)?)
    };

    let terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;

    let llm_client = LlmClient::new(&api_key, &base_url, &model);

    let app = App::new(&language, &content, &filename, llm_client);
    app.run(terminal).await;

    restore();

    Ok(())
}

fn restore() {
    ratatui::restore();
    execute!(stdout(), DisableMouseCapture).unwrap();
    execute!(stdout(), crossterm::cursor::Show).unwrap();
}

fn set_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
        std::process::exit(1);
    }));
}