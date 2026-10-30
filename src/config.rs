use anyhow::{anyhow, Result};
use chrono_tz::Tz;
use std::str::FromStr;

/// The guild where `/event` is registered. When `None`, the bot auto-detects it
/// if it is a member of exactly one guild.
pub const PREFERRED_GUILD_ID: Option<u64> = None;

/// When set, only this Discord user can run `/event`.
/// `None` means anyone in the servers the bot is in.
pub const ALLOWED_USER_ID: Option<u64> = None;

/// Timezone used for all-day and floating (timezone-less) ICS values.
pub const DEFAULT_TIMEZONE: &str = "Europe/Zurich";

#[derive(Clone, Debug)]
pub struct Config {
    pub discord_token: String,
    pub default_timezone: Tz,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();

        let discord_token = std::env::var("DISCORD_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("DISCORD_TOKEN is missing; put it in .env"))?;

        let default_timezone = Tz::from_str(DEFAULT_TIMEZONE)
            .map_err(|_| anyhow!("DEFAULT_TIMEZONE is invalid: {DEFAULT_TIMEZONE}"))?;

        Ok(Self {
            discord_token,
            default_timezone,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timezone_is_valid() {
        assert!(Tz::from_str(DEFAULT_TIMEZONE).is_ok());
    }
}
