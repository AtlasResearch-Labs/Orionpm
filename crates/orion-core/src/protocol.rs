use serde::{Deserialize, Serialize};

use crate::process_state::ProcessInfo;

/// Every request sent from the CLI to the daemon over the local socket.
/// Serialized as NDJSON (one JSON object per line).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: Method,
    pub params: serde_json::Value,
}

impl Request {
    pub fn new<P: Serialize>(id: u64, method: Method, params: P) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method,
            params: serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn parse_params<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.params.clone())
    }
}

/// Every response sent from the daemon back to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseBody {
    Result(ResponseResult),
    Error(RpcError),
}

impl Response {
    pub fn ok(id: u64, result: ResponseResult) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            body: ResponseBody::Result(result),
        }
    }

    pub fn err(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            body: ResponseBody::Error(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// Structured error payload inside a Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Standard JSON-RPC error codes plus orion-specific codes (< -32000).
    pub code: i32,
    pub message: String,
}

/// orion-specific RPC error codes.
pub mod error_codes {
    /// The named process was not found in the registry.
    pub const PROCESS_NOT_FOUND: i32 = -32001;
    /// The requested operation is not valid in the process's current state.
    pub const INVALID_STATE: i32 = -32002;
    /// The daemon is shutting down and cannot accept new requests.
    pub const DAEMON_SHUTTING_DOWN: i32 = -32003;
    /// Config parsing or validation failed.
    pub const CONFIG_ERROR: i32 = -32004;
}

/// All supported IPC methods.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    ProcessStart,
    ProcessStop,
    ProcessRestart,
    ProcessReload,
    ProcessDelete,
    ProcessList,
    ProcessInfo,
    ProcessLogs,
    DaemonStatus,
    DaemonSave,
    DaemonRestore,
    DaemonKill,
}

/// Parameters for each IPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Params {
    ProcessStart(StartParams),
    ProcessStop(NameParams),
    ProcessRestart(NameParams),
    ProcessReload(NameParams),
    ProcessDelete(NameParams),
    ProcessList(EmptyParams),
    ProcessInfo(NameParams),
    ProcessLogs(LogsParams),
    DaemonStatus(EmptyParams),
    DaemonSave(EmptyParams),
    DaemonRestore(EmptyParams),
    DaemonKill(EmptyParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmptyParams {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameParams {
    /// The name or numeric ID of the target process.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartParams {
    /// The resolved app config to pass to the daemon.
    pub config: crate::config::AppConfig,
    /// The name key to register this app under.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsParams {
    pub name: String,
    /// How many recent lines to return. Default: 50.
    #[serde(default = "default_log_lines")]
    pub lines: usize,
    /// If true, the daemon streams new lines as they arrive until the client disconnects.
    #[serde(default)]
    pub follow: bool,
}

fn default_log_lines() -> usize { 50 }

/// All possible result payloads from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum ResponseResult {
    /// Returned for process.list.
    ProcessList { processes: Vec<ProcessInfo> },

    /// Returned for process.info.
    ProcessInfo { process: ProcessInfo },

    /// Returned for process.logs (non-follow mode).
    Logs { name: String, lines: Vec<LogLine> },

    /// Returned for individual log lines in follow mode.
    LogLine { name: String, line: LogLine },

    /// Signals the end of a follow-mode log stream.
    StreamEnd,

    /// Generic acknowledgement for operations that have no meaningful return value.
    Ack { message: String },

    /// Returned for daemon.status.
    DaemonStatus {
        version: String,
        uptime_secs: u64,
        process_count: u32,
    },
}

/// A single log line captured from a managed process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// Wall-clock timestamp (Unix seconds).
    pub timestamp: u64,
    /// Which stream this came from.
    pub stream: LogStream,
    /// The raw log line text. Trailing newlines are stripped.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}
