use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
};
use dotenv::dotenv;
use ratatui_code_editor::utils::get_lang;
use std::env;
use std::fs;
use std::io::stdout;

mod app;
mod coder;
mod config;
mod diff;
mod llm;
mod prompts;
mod search;
mod tracker;
mod tree;
mod utils;
mod watcher;

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

    let (mut language, content) = if filename.is_empty() {
        (String::new(), String::new())
    } else {
        (get_lang(&filename), fs::read_to_string(&filename)?)
    };

    if language == "unknown" {
        language = "shell".to_string();
    }

    let terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;

    let llm_client = LlmClient::new(&api_key, &base_url, &model);

    let mut app = App::new(&language, &content, &filename, llm_client)?;

    if !filename.is_empty() {
        app.open_file_in_tree(&filename);
    }

    let result = app.run(terminal).await;

    restore();

    if let Err(e) = result {
        return Err(e);
    }

    Ok(())
}

fn restore() {
    ratatui::restore();
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    let _ = execute!(stdout(), crossterm::cursor::Show);
}

fn set_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
        std::process::exit(1);
    }));
}
