# TodoBot

Discord bot that imports `.ics` calendar files as Discord scheduled events.

```discord
/event file:<attachment.ics>` — upload a calendar file and create events
```

## Discord setup

1. Create an application at <https://discord.com/developers/applications>.
2. Open **Bot** and copy the token.
3. Invite the bot with the `bot` and `applications.commands` scopes and the
   **Create Events** + **Manage Events** permissions:

   ```
   https://discord.com/oauth2/authorize?client_id=YOUR_CLIENT_ID&scope=bot%20applications.commands&permissions=17600775979008
   ```

4. If the Events tab does not appear, enable Community in Server Settings.

## Configuration

The only secret lives in `.env`:

```env
DISCORD_TOKEN=...
```

Everything else is compiled into `src/config.rs`:

```rust
pub const PREFERRED_GUILD_ID: Option<u64> = None; // auto-detect one guild
pub const ALLOWED_USER_ID: Option<u64> = None;   // None = anyone in the server
pub const DEFAULT_TIMEZONE: &str = "Europe/Zurich";
```

- **Guild:** with `PREFERRED_GUILD_ID = None`, the bot registers `/event` in
  whichever guild it belongs to. If it is in more than one guild, set the ID.
- **Access:** `ALLOWED_USER_ID = None` allows anyone in the server to use the
  command. Set it to your Discord user ID to restrict it.
- **Timezone:** used for all-day and floating (timezone-less) values.

## Run locally

```bash
cp .env.example .env
# put your bot token in .env
cargo run
```

The bot must stay running while you use `/event`.

## Docker

```bash
docker compose up -d --build
docker compose logs -f
```

The image contains only Fedora, `dnf`, and the entrypoint. It does **not**
contain a Rust toolchain, source tree, or the bot binary.

On every container start, `docker-entrypoint.sh` runs:

```sh
dnf install -y --nogpgcheck \
  https://github.com/KlevisImeri/TodoBot/releases/latest/download/todobot.rpm
```

Then it drops root and runs the bot as the `todobot` user. This means every
restart picks up the latest GitHub release, but the container needs network
access at startup. `restart: unless-stopped` brings it back after a reboot or
crash, provided Docker itself starts on boot.

## Deploy

One-time setup:

```bash
cargo install cargo-generate-rpm
gh auth login
```

Then:

```bash
cargo xtask deploy
```

Bump the version in `Cargo.toml` before deploying a new release.

That command:

1. builds `target/release/todobot`
2. strips it
3. builds `dist/todobot.rpm` with `cargo generate-rpm`
4. uploads `todobot.rpm` and `todobot-linux-x86_64` to the GitHub release
5. rebuilds the Docker image
6. force-recreates the container; the entrypoint installs the latest release

Publish without touching Docker:

```bash
cargo xtask deploy --no-docker
```

Build and package locally without publishing or touching Docker:

```bash
cargo xtask package
```

This writes `dist/todobot.rpm` and `dist/todobot-linux-x86_64`.

## Usage

1. Type `/event` in Discord.
2. Select the `file` option.
3. Upload your `.ics` file.
4. Press Enter.

The bot replies privately with the created events and any warnings.

## Recurrence support

Mapped to Discord's recurrence rules:

- weekly on one weekday
- every other week
- daily on Discord's supported weekday sets (for example Mon–Fri)
- monthly on the 1st–5th weekday (for example the 4th Wednesday)
- yearly on a month/day

Not representable by Discord, so only the next occurrence is created:

- `COUNT` / `UNTIL`
- monthly by day-of-month
- weekly on multiple weekdays
- `RDATE` / `EXDATE` exceptions
- unsupported intervals and frequencies

## Tests

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
