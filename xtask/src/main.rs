use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("package") => {
            let version = package()?;
            println!("Packaged {version} into dist/");
            Ok(())
        }
        Some("deploy") => {
            let no_docker = args.any(|arg| arg == "--no-docker");
            deploy(no_docker)
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  cargo xtask package");
            eprintln!("  cargo xtask deploy [--no-docker]");
            Ok(())
        }
    }
}

fn package() -> Result<String> {
    let version = package_version()?;

    ensure_tool("cargo-generate-rpm", "cargo install cargo-generate-rpm")?;

    run("cargo", &["build", "--release", "-p", "todobot"])?;
    run("strip", &["-s", "target/release/todobot"])?;
    run("cargo", &["generate-rpm"])?;

    fs::create_dir_all("dist").context("failed to create dist/")?;

    let rpm = find_rpm(&version)?;
    fs::copy(&rpm, "dist/todobot.rpm")
        .with_context(|| format!("failed to copy {}", rpm.display()))?;

    let binary = Path::new("target/release/todobot");
    if !binary.exists() {
        bail!("release binary is missing at {}", binary.display());
    }
    fs::copy(binary, "dist/todobot-linux-x86_64").context("failed to copy the release binary")?;

    Ok(version)
}

fn deploy(no_docker: bool) -> Result<()> {
    ensure_tool("gh", "install the GitHub CLI")?;
    ensure_gh_auth()?;
    if !no_docker {
        ensure_tool("docker", "install Docker")?;
        if !Path::new(".env").exists() {
            bail!(".env is missing; run `cp .env.example .env` and add DISCORD_TOKEN");
        }
    }

    let version = package()?;
    let tag = format!("v{version}");

    let release_exists = Command::new("gh")
        .args(["release", "view", &tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run gh")?
        .success();

    if release_exists {
        run(
            "gh",
            &[
                "release",
                "upload",
                &tag,
                "dist/todobot.rpm",
                "dist/todobot-linux-x86_64",
                "--clobber",
            ],
        )?;
    } else {
        run(
            "gh",
            &[
                "release",
                "create",
                &tag,
                "dist/todobot.rpm",
                "dist/todobot-linux-x86_64",
                "--title",
                &tag,
                "--notes",
                &format!("TodoBot {tag}"),
            ],
        )?;
    }

    if !no_docker {
        // Give GitHub a moment to make the new release asset available.
        std::thread::sleep(Duration::from_secs(2));
        run("docker", &["compose", "build"])?;
        run("docker", &["compose", "up", "-d", "--force-recreate"])?;
        println!("Deployed {tag} and restarted Docker");
    } else {
        println!("Published {tag} (Docker not touched)");
    }

    Ok(())
}

fn package_version() -> Result<String> {
    let content = fs::read_to_string("Cargo.toml").context("failed to read Cargo.toml")?;
    let manifest: toml::Value = toml::from_str(&content).context("failed to parse Cargo.toml")?;
    manifest
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_string)
        .context("package.version is missing from Cargo.toml")
}

fn find_rpm(version: &str) -> Result<PathBuf> {
    let prefix = format!("todobot-{version}-");
    let directory = Path::new("target/generate-rpm");
    let entry = fs::read_dir(directory)
        .with_context(|| format!("failed to read {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".rpm"))
        })
        .with_context(|| {
            format!(
                "cargo generate-rpm did not produce an RPM for version {version} in {}",
                directory.display()
            )
        })?;
    Ok(entry)
}

fn ensure_tool(tool: &str, install_hint: &str) -> Result<()> {
    let status = Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => bail!("{tool} is not available; {install_hint}"),
    }
}

fn ensure_gh_auth() -> Result<()> {
    let status = Command::new("gh")
        .args(["auth", "status"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run gh auth status")?;
    if !status.success() {
        bail!("gh is not authenticated; run `gh auth login`");
    }
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    println!("$ {} {}", program, args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}
