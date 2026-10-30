mod commands;
mod config;
mod discord_events;
mod ics;
mod model;
mod recurrence;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use poise::serenity_prelude as serenity;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!("{error}");
            std::process::exit(1);
        }
    };

    let api = discord_events::DiscordApi::new(config.discord_token.clone());

    let guild_id = match resolve_guild(&api).await {
        Ok(guild_id) => guild_id,
        Err(error) => {
            tracing::error!("{error:#}");
            std::process::exit(1);
        }
    };

    let data = Arc::new(commands::Data { config, api });

    let data_for_framework = data.clone();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![commands::event::event()],
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            let data = data_for_framework.clone();
            Box::pin(async move {
                poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id)
                    .await?;
                tracing::info!("Registered /event in guild {guild_id}");
                Ok((*data).clone())
            })
        })
        .build();

    let mut client = serenity::Client::builder(
        data.config.discord_token.clone(),
        serenity::GatewayIntents::non_privileged(),
    )
    .framework(framework)
    .await
    .expect("failed to create the Discord client");

    if let Err(error) = client.start().await {
        tracing::error!("Bot failed: {error}");
        std::process::exit(1);
    }
}

async fn resolve_guild(api: &discord_events::DiscordApi) -> anyhow::Result<serenity::GuildId> {
    if let Some(guild_id) = config::PREFERRED_GUILD_ID {
        return Ok(serenity::GuildId::new(guild_id));
    }

    let guilds = api
        .list_guilds()
        .await
        .context("failed to list the bot's servers")?;

    match guilds.as_slice() {
        [guild] => {
            let id: u64 = guild
                .id
                .parse()
                .context("Discord returned an invalid guild ID")?;
            tracing::info!("Using guild {} ({})", guild.name, guild.id);
            Ok(serenity::GuildId::new(id))
        }
        [] => bail!("the bot is not in any server; invite it first"),
        _ => {
            let list = guilds
                .iter()
                .map(|guild| format!("{} ({})", guild.name, guild.id))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "the bot is in multiple servers ({list}); set PREFERRED_GUILD_ID in src/config.rs"
            )
        }
    }
}
