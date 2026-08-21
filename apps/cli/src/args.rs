use std::ffi::OsString;

use deepagent_models::DEEPSEEK_OFFICIAL_PROVIDER;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Run(RunOptions),
    ToolsList,
    SandboxStatus,
    Server { transport: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub prompt: String,
    pub continue_thread: Option<String>,
    pub json: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub sandbox_backend: Option<String>,
    pub permission_profile: Option<String>,
    pub reasoning_effort: Option<String>,
}

pub fn parse_args<I, S>(args: I) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _binary = args.next();
    let command = args
        .next()
        .ok_or_else(|| usage().to_string())?
        .to_string_lossy()
        .to_string();

    match command.as_str() {
        "run" => parse_run(args, None),
        "resume" => {
            let thread_id = args
                .next()
                .ok_or_else(|| "resume requires a thread id".to_string())?
                .to_string_lossy()
                .to_string();
            parse_run(args, Some(thread_id))
        }
        "tools" => {
            expect_subcommand(&mut args, "list")?;
            ensure_no_args(args)?;
            Ok(CliCommand::ToolsList)
        }
        "sandbox" => {
            expect_subcommand(&mut args, "status")?;
            ensure_no_args(args)?;
            Ok(CliCommand::SandboxStatus)
        }
        "server" => {
            let mut transport = None;
            let mut rest = args.peekable();
            while let Some(arg) = rest.next() {
                let arg = arg.to_string_lossy().to_string();
                if arg == "--transport" {
                    transport = Some(
                        rest.next()
                            .ok_or_else(|| "--transport requires a value".to_string())?
                            .to_string_lossy()
                            .to_string(),
                    );
                } else {
                    return Err(format!("unknown server option: {arg}"));
                }
            }
            let transport = transport.unwrap_or_else(|| "stdio".to_string());
            if transport != "stdio" {
                return Err(format!("unsupported server transport: {transport}"));
            }
            Ok(CliCommand::Server { transport })
        }
        "-h" | "--help" => Err(usage().to_string()),
        other => Err(format!("unknown command '{other}'\n\n{}", usage())),
    }
}

fn parse_run<I, S>(args: I, continue_thread: Option<String>) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into).peekable();
    let mut json = false;
    let mut provider = None;
    let mut model = None;
    let mut sandbox_backend = None;
    let mut permission_profile = None;
    let mut reasoning_effort = None;
    let mut prompt_parts = Vec::new();

    while let Some(raw) = args.next() {
        let value = raw.to_string_lossy().to_string();
        match value.as_str() {
            "--json" => json = true,
            "--provider" => provider = Some(next_value(&mut args, "--provider")?),
            "--model" => model = Some(next_value(&mut args, "--model")?),
            "--sandbox-backend" => {
                sandbox_backend = Some(next_value(&mut args, "--sandbox-backend")?)
            }
            "--permission-profile" => {
                permission_profile = Some(next_value(&mut args, "--permission-profile")?)
            }
            "--reasoning-effort" => {
                reasoning_effort = Some(next_value(&mut args, "--reasoning-effort")?)
            }
            "--" => {
                prompt_parts.extend(args.by_ref().map(|item| item.to_string_lossy().to_string()));
                break;
            }
            value if value.starts_with('-') => return Err(format!("unknown run option: {value}")),
            value => prompt_parts.push(value.to_string()),
        }
    }

    if provider
        .as_deref()
        .is_some_and(|provider| provider != DEEPSEEK_OFFICIAL_PROVIDER)
    {
        return Err(format!(
            "unsupported provider; only '{DEEPSEEK_OFFICIAL_PROVIDER}' is available"
        ));
    }
    if continue_thread.is_none() && prompt_parts.is_empty() {
        return Err("run requires a prompt".to_string());
    }

    Ok(CliCommand::Run(RunOptions {
        prompt: prompt_parts.join(" "),
        continue_thread,
        json,
        provider,
        model,
        sandbox_backend,
        permission_profile,
        reasoning_effort,
    }))
}

fn next_value<I>(args: &mut std::iter::Peekable<I>, option: &str) -> Result<String, String>
where
    I: Iterator,
    I::Item: Into<OsString>,
{
    args.next()
        .map(Into::into)
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{option} requires a value"))
}

fn expect_subcommand<I, S>(args: &mut I, expected: &str) -> Result<(), String>
where
    I: Iterator<Item = S>,
    S: Into<OsString>,
{
    let actual = args
        .next()
        .ok_or_else(|| format!("expected '{expected}'"))?
        .into()
        .to_string_lossy()
        .to_string();
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected '{expected}', got '{actual}'"))
    }
}

fn ensure_no_args<I, S>(args: I) -> Result<(), String>
where
    I: Iterator<Item = S>,
    S: Into<OsString>,
{
    if let Some(value) = args.into_iter().next() {
        return Err(format!(
            "unexpected argument: {}",
            value.into().to_string_lossy()
        ));
    }
    Ok(())
}

fn usage() -> &'static str {
    "usage: deepagent <run|resume|tools list|sandbox status|server> [options]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_run_with_provider_model_and_security_options() {
        let command = parse_args([
            "deepagent",
            "run",
            "--json",
            "--provider",
            "deepseek-official",
            "--model",
            "deepseek-v4-pro",
            "--sandbox-backend",
            "windows_sandbox",
            "--permission-profile",
            "workspace_write",
            "inspect the repository",
        ])
        .unwrap();

        assert_eq!(
            command,
            CliCommand::Run(RunOptions {
                prompt: "inspect the repository".into(),
                continue_thread: None,
                json: true,
                provider: Some("deepseek-official".into()),
                model: Some("deepseek-v4-pro".into()),
                sandbox_backend: Some("windows_sandbox".into()),
                permission_profile: Some("workspace_write".into()),
                reasoning_effort: None,
            })
        );
    }

    #[test]
    fn parses_resume_with_json_output() {
        assert_eq!(
            parse_args(["deepagent", "resume", "thread-1", "--json"]).unwrap(),
            CliCommand::Run(RunOptions {
                prompt: String::new(),
                continue_thread: Some("thread-1".into()),
                json: true,
                provider: None,
                model: None,
                sandbox_backend: None,
                permission_profile: None,
                reasoning_effort: None,
            })
        );
    }

    #[test]
    fn rejects_unknown_provider_before_running() {
        let error = parse_args(["deepagent", "run", "--provider", "openai", "hello"]).unwrap_err();
        assert!(error.contains("unsupported provider"));
    }

    #[test]
    fn parses_tools_and_sandbox_commands() {
        assert_eq!(
            parse_args(["deepagent", "tools", "list"]).unwrap(),
            CliCommand::ToolsList
        );
        assert_eq!(
            parse_args(["deepagent", "sandbox", "status"]).unwrap(),
            CliCommand::SandboxStatus
        );
    }
}
