pub mod client;
pub mod server;

pub const SOCKET_NAME: &str = "orion-daemon.sock";

use interprocess::local_socket::Name;

#[cfg(windows)]
pub fn get_socket_name() -> anyhow::Result<Name<'static>> {
    use interprocess::local_socket::ToNsName;
    Ok(SOCKET_NAME.to_ns_name::<interprocess::local_socket::GenericNamespaced>()?)
}

#[cfg(not(windows))]
pub fn get_socket_path() -> std::path::PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = std::path::PathBuf::from(runtime_dir).join(SOCKET_NAME);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        return p;
    }
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("orion-daemon-{}.sock", uid))
}

#[cfg(not(windows))]
pub fn get_socket_name() -> anyhow::Result<Name<'static>> {
    use interprocess::local_socket::ToFsName;
    let path = get_socket_path();
    Ok(path.to_fs_name::<interprocess::local_socket::GenericFilePath>()?)
}
