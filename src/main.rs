use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        if let Some(config) = daemon_config_from_env()? {
            return ts_bridge::run_daemon_server(config);
        }
        return ts_bridge::run_stdio_server();
    };

    match first.as_str() {
        "daemon" => {
            let config = parse_daemon_args(args)?;
            ts_bridge::run_daemon_server(config)
        }
        "-V" | "--version" => {
            print_version();
            Ok(())
        }
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown subcommand {other}")),
    }
}

fn daemon_config_from_env() -> anyhow::Result<Option<ts_bridge::DaemonConfig>> {
    let Some(value) = std::env::var("TS_BRIDGE_DAEMON").ok() else {
        return Ok(None);
    };
    if !parse_env_bool("TS_BRIDGE_DAEMON", &value)? {
        return Ok(None);
    }

    let mut config = ts_bridge::DaemonConfig::default();
    if let Ok(listen) = std::env::var("TS_BRIDGE_DAEMON_LISTEN") {
        config.listen = Some(listen.parse()?);
    }
    if let Ok(socket) = std::env::var("TS_BRIDGE_DAEMON_SOCKET") {
        config.socket = Some(socket.into());
    }
    if let Ok(idle_ttl) = std::env::var("TS_BRIDGE_DAEMON_IDLE_TTL") {
        config.idle_ttl =
            parse_idle_ttl(&idle_ttl).with_context(|| "parse TS_BRIDGE_DAEMON_IDLE_TTL")?;
    }
    Ok(Some(config))
}

fn parse_env_bool(name: &str, value: &str) -> anyhow::Result<bool> {
    let lowered = value.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(anyhow::anyhow!(
            "{name} must be one of 1,true,yes,on,0,false,no,off"
        )),
    }
}

fn parse_idle_ttl(value: &str) -> anyhow::Result<Option<std::time::Duration>> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("off") || trimmed == "0" {
        return Ok(None);
    }
    let (number, unit) = match trimmed.chars().last() {
        Some('s') => (&trimmed[..trimmed.len() - 1], 1),
        Some('m') => (&trimmed[..trimmed.len() - 1], 60),
        Some('h') => (&trimmed[..trimmed.len() - 1], 3600),
        _ => (trimmed, 1),
    };
    let amount: u64 = number
        .parse()
        .with_context(|| "daemon idle TTL must be a number of seconds or use s/m/h suffix")?;
    let seconds = amount.saturating_mul(unit);
    Ok(Some(std::time::Duration::from_secs(seconds)))
}

fn parse_daemon_args<I>(mut args: I) -> anyhow::Result<ts_bridge::DaemonConfig>
where
    I: Iterator<Item = String>,
{
    let mut config = ts_bridge::DaemonConfig::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--listen requires HOST:PORT"))?;
                config.listen = Some(value.parse()?);
            }
            "--socket" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--socket requires PATH"))?;
                config.socket = Some(value.into());
            }
            "--idle-ttl" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--idle-ttl requires SECONDS|off"))?;
                config.idle_ttl = parse_idle_ttl(&value).with_context(|| "parse --idle-ttl")?;
            }
            "-h" | "--help" => {
                print_daemon_usage();
                std::process::exit(0);
            }
            _ if arg.starts_with("--listen=") => {
                let value = arg.trim_start_matches("--listen=");
                config.listen = Some(value.parse()?);
            }
            _ if arg.starts_with("--socket=") => {
                let value = arg.trim_start_matches("--socket=");
                config.socket = Some(value.into());
            }
            _ if arg.starts_with("--idle-ttl=") => {
                let value = arg.trim_start_matches("--idle-ttl=");
                config.idle_ttl = parse_idle_ttl(value).with_context(|| "parse --idle-ttl")?;
            }
            other => return Err(anyhow::anyhow!("unknown daemon flag {other}")),
        }
    }
    Ok(config)
}

fn print_usage() {
    eprintln!(
        "Usage:\n  ts-bridge\n  ts-bridge daemon [--listen HOST:PORT] [--socket PATH] [--idle-ttl SECONDS|off]\n"
    );
}

fn print_daemon_usage() {
    eprintln!(
        "Usage:\n  ts-bridge daemon [--listen HOST:PORT] [--socket PATH] [--idle-ttl SECONDS|off]\n"
    );
}

fn print_version() {
    println!("ts-bridge {}", env!("CARGO_PKG_VERSION"));
}
