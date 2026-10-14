use anyhow::Result;
use clap::Args;
use orion_core::protocol::{
    LogLine, LogStream, LogsParams, Method, Params, Request, ResponseBody, ResponseResult,
};

use super::client;

#[derive(Args)]
pub struct LogsArgs {
    /// Name or numeric ID of the process (or "all" for all processes).
    pub target: String,

    /// Number of recent lines to display.
    #[arg(long, short = 'n', default_value = "50")]
    pub lines: usize,

    /// Stream new log lines as they arrive (tail mode).
    #[arg(long, short = 'f')]
    pub follow: bool,
}

pub async fn run(args: LogsArgs) -> Result<()> {
    client::ensure_daemon().await?;

    let req = Request::new(
        1,
        Method::ProcessLogs,
        Params::ProcessLogs(LogsParams {
            name: args.target.clone(),
            lines: args.lines,
            follow: args.follow,
        }),
    );

    if args.follow {
        orion_ipc::client::stream_logs(&req, |response| {
            if let ResponseBody::Result(ResponseResult::LogLine { name, line }) = response.body {
                print_log_line(&name, &line);
            }
        })
        .await?;
    } else {
        let result = client::rpc(
            Method::ProcessLogs,
            Params::ProcessLogs(LogsParams {
                name: args.target.clone(),
                lines: args.lines,
                follow: false,
            }),
        )
        .await?;

        if let ResponseResult::Logs { name, lines } = result {
            for line in lines {
                print_log_line(&name, &line);
            }
        }
    }

    Ok(())
}

fn print_log_line(name: &str, line: &LogLine) {
    // Convert Unix timestamp to UTC date+time string.
    // We avoid pulling in chrono; derive it manually from the epoch.
    let ts = line.timestamp;
    let secs_per_day: u64 = 86400;
    let days_since_epoch = ts / secs_per_day;
    let time_of_day = ts % secs_per_day;

    // Days-since-epoch → calendar date (Gregorian proleptic).
    // Algorithm: https://howardhinnant.github.io/date_algorithms.html (civil_from_days)
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

    let stream = match line.stream {
        LogStream::Stdout => "out",
        LogStream::Stderr => "err",
    };

    println!("[{}] [{}] [{}] {}", dt, name, stream, line.text);
}

