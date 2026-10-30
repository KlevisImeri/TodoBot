pub mod event;

use crate::config::Config;
use crate::discord_events::DiscordApi;

#[derive(Clone)]
pub struct Data {
    pub config: Config,
    pub api: DiscordApi,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
