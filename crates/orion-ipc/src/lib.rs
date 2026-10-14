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
pub fn get_socket_name() -> anyhow::Result<Name<'static>> {
    use interprocess::local_socket::ToFsName;
    let path = std::env::temp_dir().join(SOCKET_NAME);
    Ok(path.to_fs_name::<interprocess::local_socket::GenericFilePath>()?)
}
