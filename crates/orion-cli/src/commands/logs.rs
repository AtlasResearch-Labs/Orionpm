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
    let secs = line.timestamp % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let dt = format!("{:02}:{:02}:{:02}", h, m, s);

    let stream = match line.stream {
        LogStream::Stdout => "out",
        LogStream::Stderr => "err",
    };

    println!("[{}] [{}] [{}] {}", dt, name, stream, line.text);
}
