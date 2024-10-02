cfg_net! {
    use crate::net::addr::{self, ToSocketAddrs};

    use std::io;
    use std::net::SocketAddr;

    /// Performs a DNS resolution.
    pub async fn lookup_host<T: ToSocketAddrs>(host: T) -> io::Result<impl Iterator<Item = SocketAddr>> {
        addr::to_socket_addrs(host).await
    }
}
