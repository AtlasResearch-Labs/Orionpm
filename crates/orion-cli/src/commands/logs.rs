use anyhow::Result;
use clap::Args;
use orion_core::protocol::{
    LogLine, LogStream, LogsParams, Method, Params, Request, ResponseBody, ResponseResult,
};

use super::client;

#[derive(Args)]
pub struct LogsArgs {
    /// Name or numeric ID of the process, or "all" for all processes [default: all].
    #[arg(default_value = "all")]
    pub target: String,

    /// Number of recent lines to display per process.
    #[arg(long, short = 'n', default_value = "15")]
    pub lines: usize,

    /// Stream new log lines as they arrive (default behavior).
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Output recent log lines and exit without streaming.
    #[arg(long)]
    pub nostream: bool,

    /// Raw log output without timestamps or process prefixes.
    #[arg(long)]
    pub raw: bool,
}

pub async fn run(args: LogsArgs) -> Result<()> {
    client::ensure_daemon().await?;

    let follow = !args.nostream;

    if !args.raw {
        let stream_hint = if follow { ", Ctrl+C to exit" } else { "" };
        println!(
            "\x1b[1;36m[TAILING]\x1b[0m Tailing last {} lines for [{}] (change with --lines <N>{})",
            args.lines, args.target, stream_hint
        );
    }

    let req = Request::new(
        1,
        Method::ProcessLogs,
        Params::ProcessLogs(LogsParams {
            name: args.target.clone(),
            lines: args.lines,
            follow,
        }),
    );

    let raw = args.raw;
    orion_ipc::client::stream_logs(&req, move |response| {
        if let ResponseBody::Result(ResponseResult::LogLine { name, line }) = response.body {
            print_log_line(&name, &line, raw);
        }
    })
    .await?;

    Ok(())
}

fn get_proc_color(name: &str) -> &'static str {
    const COLORS: &[&str] = &[
        "36", // Cyan
        "32", // Green
        "33", // Yellow
        "35", // Magenta
        "34", // Blue
        "96", // Bright Cyan
        "92", // Bright Green
        "95", // Bright Magenta
    ];
    let hash: usize = name.bytes().map(|b| b as usize).sum();
    COLORS[hash % COLORS.len()]
}

fn print_log_line(name: &str, line: &LogLine, raw: bool) {
    if raw {
        println!("{}", line.text);
        return;
    }

    let ts = line.timestamp;
    let secs_per_day: u64 = 86400;
    let days_since_epoch = ts / secs_per_day;
    let time_of_day = ts % secs_per_day;

    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let h = time_of_day / 3600;
    let min = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    let dt = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, h, min, s);
    let color = get_proc_color(name);

    match line.stream {
        LogStream::Stdout => {
            println!(
                "\x1b[90m{}\x1b[0m \x1b[1;{}m{:<14}\x1b[0m \x1b[90m[out] │\x1b[0m {}",
                dt, color, name, line.text
            );
        }
        LogStream::Stderr => {
            println!(
                "\x1b[90m{}\x1b[0m \x1b[1;31m{:<14}\x1b[0m \x1b[1;31m[err] │\x1b[0m \x1b[31m{}\x1b[0m",
                dt, name, line.text
            );
        }
    }
}
