use std::net::IpAddr;

pub struct ConnInfo {
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub local_port: u16,
    pub local_addr: String,
}
