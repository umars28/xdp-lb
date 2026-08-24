use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};

use tokio::{net::TcpStream, time::timeout};

pub async fn probe_tcp(address: Ipv4Addr, port: u16, budget: Duration) -> bool {
    let target = SocketAddr::V4(SocketAddrV4::new(address, port));
    matches!(timeout(budget, TcpStream::connect(target)).await, Ok(Ok(_)))
}
