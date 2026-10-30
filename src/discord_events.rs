use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::model::{CreateScheduledEventPayload, PartialGuild, ScheduledEvent};

const USER_AGENT: &str = concat!(
    "DiscordBot (https://github.com/KlevisImeri/TodoBot, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

#[derive(Clone)]
pub struct DiscordApi {
    http: Client,
    token: String,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    retry_after: Option<f64>,
}

impl DiscordApi {
    pub fn new(token: impl Into<String>) -> Self {
        let base_url = std::env::var("DISCORD_API_BASE")
            .unwrap_or_else(|_| "https://discord.com/api/v10".to_string());
        Self::with_base_url(token, base_url)
    }

    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build the HTTP client");
        Self {
            http,
            token: token.into(),
            base_url: base_url.into(),
        }
    }

    pub async fn list_guilds(&self) -> Result<Vec<PartialGuild>> {
        self.request(Method::GET, "/users/@me/guilds", None::<&()>)
            .await
    }

    pub async fn list_scheduled_events(&self, guild_id: u64) -> Result<Vec<ScheduledEvent>> {
        self.request(
            Method::GET,
            &format!("/guilds/{guild_id}/scheduled-events"),
            None::<&()>,
        )
        .await
    }

    pub async fn count_active_events(&self, guild_id: u64) -> Result<usize> {
        let events = self.list_scheduled_events(guild_id).await?;
        Ok(events
            .iter()
            .filter(|event| event.status == 1 || event.status == 2)
            .count())
    }

    pub async fn create_scheduled_event(
        &self,
        guild_id: u64,
        payload: &CreateScheduledEventPayload,
    ) -> Result<ScheduledEvent> {
        self.request(
            Method::POST,
            &format!("/guilds/{guild_id}/scheduled-events"),
            Some(payload),
        )
        .await
    }

    async fn request<T, B>(&self, method: Method, path: &str, body: Option<&B>) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let mut last_error = None;

        for _ in 0..3 {
            let mut request = self
                .http
                .request(method.clone(), &url)
                .header(
                    reqwest::header::AUTHORIZATION,
                    format!("Bot {}", self.token),
                )
                .header(reqwest::header::ACCEPT, "application/json");

            if let Some(body) = body {
                request = request.json(body);
            }

            let response = request
                .send()
                .await
                .with_context(|| format!("request to {url} failed"))?;
            let status = response.status();

            if status == StatusCode::TOO_MANY_REQUESTS {
                let header_retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<f64>().ok());
                let retry_after = response
                    .json::<RateLimitResponse>()
                    .await
                    .ok()
                    .and_then(|value| value.retry_after)
                    .or(header_retry_after)
                    .unwrap_or(1.0);
                tokio::time::sleep(Duration::from_secs_f64(retry_after.max(0.25))).await;
                last_error = Some(anyhow!("Discord rate limited the request"));
                continue;
            }

            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                return Err(discord_error(status, &text));
            }

            return response
                .json::<T>()
                .await
                .with_context(|| format!("failed to decode Discord response from {url}"));
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Discord rate limited the request")))
    }
}

fn discord_error(status: StatusCode, body: &str) -> anyhow::Error {
    #[derive(Deserialize)]
    struct ErrorBody {
        message: Option<String>,
        errors: Option<serde_json::Value>,
    }

    let parsed: Option<ErrorBody> = serde_json::from_str(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value.message.clone())
        .unwrap_or_else(|| body.to_string());

    let mut output = match status {
        StatusCode::UNAUTHORIZED => {
            format!("Discord rejected the bot token (401). {message}")
        }
        StatusCode::FORBIDDEN => {
            format!("The bot is missing access or the Create Events permission (403). {message}")
        }
        StatusCode::BAD_REQUEST => {
            format!("Discord rejected the event payload (400). {message}")
        }
        _ => format!("Discord API error ({status}): {message}"),
    };

    if let Some(errors) = parsed.and_then(|value| value.errors) {
        if let Ok(pretty) = serde_json::to_string_pretty(&errors) {
            output.push('\n');
            output.push_str(&pretty);
        }
    }

    anyhow!(output)
}

pub fn event_url(guild_id: u64, event_id: &str) -> String {
    format!("https://discord.com/events/{guild_id}/{event_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiscordRecurrenceRule, EntityMetadata};
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn payload() -> CreateScheduledEventPayload {
        CreateScheduledEventPayload {
            name: "Team meeting".to_string(),
            description: Some("Discuss the roadmap".to_string()),
            scheduled_start_time: "2026-11-02T10:00:00Z".to_string(),
            scheduled_end_time: "2026-11-02T11:00:00Z".to_string(),
            privacy_level: 2,
            entity_type: 3,
            entity_metadata: EntityMetadata {
                location: "Office".to_string(),
            },
            recurrence_rule: Some(DiscordRecurrenceRule {
                start: "2026-11-02T10:00:00Z".to_string(),
                frequency: 2,
                interval: 1,
                by_weekday: Some(vec![0]),
                by_n_weekday: None,
                by_month: None,
                by_month_day: None,
            }),
        }
    }

    #[tokio::test]
    async fn creates_an_event_with_the_expected_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/guilds/123/scheduled-events"))
            .and(header("authorization", "Bot test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "42",
                "name": "Team meeting",
                "scheduled_start_time": "2026-11-02T10:00:00+00:00",
                "scheduled_end_time": "2026-11-02T11:00:00+00:00",
                "status": 1
            })))
            .mount(&server)
            .await;

        let api = DiscordApi::with_base_url("test-token", server.uri());
        let event = api.create_scheduled_event(123, &payload()).await.unwrap();
        assert_eq!(event.id, "42");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["name"], "Team meeting");
        assert_eq!(body["entity_type"], 3);
        assert_eq!(body["entity_metadata"]["location"], "Office");
        assert_eq!(body["recurrence_rule"]["frequency"], 2);
        assert_eq!(body["recurrence_rule"]["by_weekday"], json!([0]));
    }

    #[tokio::test]
    async fn counts_only_scheduled_and_active_events() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/guilds/123/scheduled-events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": "1", "status": 1 },
                { "id": "2", "status": 2 },
                { "id": "3", "status": 3 },
                { "id": "4", "status": 4 }
            ])))
            .mount(&server)
            .await;

        let api = DiscordApi::with_base_url("test-token", server.uri());
        assert_eq!(api.count_active_events(123).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn surfaces_authentication_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me/guilds"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "401: Unauthorized",
                "code": 0
            })))
            .mount(&server)
            .await;

        let api = DiscordApi::with_base_url("bad-token", server.uri());
        let error = api.list_guilds().await.unwrap_err().to_string();
        assert!(error.contains("401"));
        assert!(error.contains("Unauthorized"));
    }

    #[tokio::test]
    async fn retries_after_a_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me/guilds"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "retry_after": 0.01,
                "global": false
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users/@me/guilds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": "123", "name": "Todo server" }
            ])))
            .mount(&server)
            .await;

        let api = DiscordApi::with_base_url("test-token", server.uri());
        let guilds = api.list_guilds().await.unwrap();
        assert_eq!(guilds.len(), 1);
        assert_eq!(guilds[0].name, "Todo server");
    }
}
