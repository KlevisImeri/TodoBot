use anyhow::{bail, Context as _};
use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use poise::CreateReply;
use serenity::{Attachment, Colour, CreateEmbed};

use super::{Context, Error};
use crate::ics::{parse_calendar, ParseOptions};
use crate::model::truncate;

const MAX_ICS_BYTES: u64 = 5 * 1024 * 1024;

/// Import events from an .ics calendar file.
#[poise::command(slash_command, guild_only)]
pub async fn event(
    ctx: Context<'_>,
    #[description = "The .ics calendar file"] file: Attachment,
) -> Result<(), Error> {
    if let Some(allowed) = crate::config::ALLOWED_USER_ID {
        if ctx.author().id.get() != allowed {
            ctx.send(
                CreateReply::default()
                    .content("Only the configured user can use this command.")
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    }

    let Some(guild_id) = ctx.guild_id() else {
        ctx.send(
            CreateReply::default()
                .content("This command only works inside a server.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    ctx.defer_ephemeral().await?;

    match import(ctx, guild_id, file).await {
        Ok(report) => {
            ctx.send(
                CreateReply::default()
                    .embed(report_embed(&report))
                    .ephemeral(true),
            )
            .await?;
        }
        Err(error) => {
            tracing::error!("event import failed: {error:#}");
            let message = truncate(&format!("Import failed: {error:#}"), 1900);
            ctx.send(CreateReply::default().content(message).ephemeral(true))
                .await?;
        }
    }

    Ok(())
}

struct CreatedEvent {
    name: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    all_day: bool,
    recurrence_label: Option<String>,
    url: String,
}

#[derive(Default)]
struct ImportReport {
    created: Vec<CreatedEvent>,
    failed: Vec<(String, String)>,
    skipped: Vec<String>,
    warnings: Vec<String>,
}

async fn import(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    file: Attachment,
) -> anyhow::Result<ImportReport> {
    validate_attachment(&file)?;

    let bytes = file
        .download()
        .await
        .context("failed to download the attachment")?;
    if bytes.len() as u64 > MAX_ICS_BYTES {
        bail!("the file is too large (max 5 MiB)");
    }

    let content = std::str::from_utf8(&bytes).context("the .ics file is not valid UTF-8")?;
    let parsed = parse_calendar(
        content,
        &ParseOptions {
            default_timezone: ctx.data().config.default_timezone,
            now: Utc::now(),
        },
    )?;

    let mut report = ImportReport {
        created: Vec::new(),
        failed: Vec::new(),
        skipped: parsed.skipped,
        warnings: parsed.warnings,
    };

    if parsed.events.is_empty() {
        return Ok(report);
    }

    let current = ctx.data().api.count_active_events(guild_id.get()).await?;
    if current + parsed.events.len() > 100 {
        bail!(
            "this would exceed Discord's limit of 100 active/scheduled events \
             in this server (currently {current})"
        );
    }

    for draft in parsed.events {
        match ctx
            .data()
            .api
            .create_scheduled_event(guild_id.get(), &draft.payload())
            .await
        {
            Ok(event) => {
                report.created.push(CreatedEvent {
                    name: draft.name,
                    start: draft.start,
                    end: draft.end,
                    all_day: draft.all_day,
                    recurrence_label: draft.recurrence_label,
                    url: crate::discord_events::event_url(guild_id.get(), &event.id),
                });
            }
            Err(error) => report.failed.push((draft.name, error.to_string())),
        }
    }

    Ok(report)
}

fn validate_attachment(file: &Attachment) -> anyhow::Result<()> {
    if u64::from(file.size) > MAX_ICS_BYTES {
        bail!("the file is too large (max 5 MiB)");
    }

    let looks_like_ics = file.filename.to_ascii_lowercase().ends_with(".ics")
        || file
            .content_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("text/calendar"));

    if !looks_like_ics {
        bail!("please upload a .ics calendar file");
    }

    Ok(())
}

fn report_embed(report: &ImportReport) -> CreateEmbed {
    let mut lines = Vec::new();

    for event in &report.created {
        let when = if event.all_day {
            format!("<t:{}:D>", event.start.timestamp())
        } else if event.start.date_naive() == event.end.date_naive() {
            format!(
                "<t:{}:F> – <t:{}:t>",
                event.start.timestamp(),
                event.end.timestamp()
            )
        } else {
            format!(
                "<t:{}:F> – <t:{}:F>",
                event.start.timestamp(),
                event.end.timestamp()
            )
        };

        let mut line = format!("**{}**\n{}\n[View event]({})", event.name, when, event.url);
        if let Some(label) = &event.recurrence_label {
            line.push_str(&format!("\n🔁 {label}"));
        }
        lines.push(line);
    }

    let mut description = lines.join("\n\n");

    if !report.skipped.is_empty() {
        description.push_str("\n\n**Skipped**\n");
        description.push_str(&bullets(&report.skipped));
    }
    if !report.failed.is_empty() {
        description.push_str("\n\n**Failed**\n");
        description.push_str(
            &report
                .failed
                .iter()
                .map(|(name, error)| format!("• {name}: {error}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !report.warnings.is_empty() {
        description.push_str("\n\n**Notes**\n");
        description.push_str(&bullets(&report.warnings));
    }

    if description.is_empty() {
        description = "No events were created.".to_string();
    }

    let title = match (report.created.len(), report.failed.len()) {
        (0, _) => "No events created".to_string(),
        (count, 0) => format!(
            "Imported {count} event{}",
            if count == 1 { "" } else { "s" }
        ),
        (count, failed) => format!("Imported {count} event(s), {failed} failed"),
    };

    let colour = if report.failed.is_empty() {
        Colour::new(0x57F287)
    } else {
        Colour::new(0xFEE75C)
    };

    CreateEmbed::new()
        .title(title)
        .description(truncate(&description, 4096))
        .color(colour)
}

fn bullets(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("• {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}
