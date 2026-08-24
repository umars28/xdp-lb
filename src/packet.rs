use std::net::Ipv4Addr;

pub const ETH_LEN: usize = 14;
pub const IP_LEN: usize = 20;
pub const TCP_LEN: usize = 20;

pub const PROTO_TCP: u8 = 6;
pub const PROTO_IPIP: u8 = 4;
pub const ETH_P_IP: u16 = 0x0800;
pub const ETH_P_ARP: u16 = 0x0806;

pub fn ones_complement(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut index = 0;

    while index + 1 < bytes.len() {
        sum += u16::from_be_bytes([bytes[index], bytes[index + 1]]) as u32;
        index += 2;
    }
    if index < bytes.len() {
        sum += (bytes[index] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    pub mac: [u8; 6],
    pub address: Ipv4Addr,
    pub port: u16,
}

pub fn tcp_syn(from: Endpoint, to: Endpoint) -> Vec<u8> {
    let total_len = IP_LEN + TCP_LEN;
    let mut packet = Vec::with_capacity(ETH_LEN + total_len);

    packet.extend_from_slice(&to.mac);
    packet.extend_from_slice(&from.mac);
    packet.extend_from_slice(&ETH_P_IP.to_be_bytes());

    packet.push(0x45);
    packet.push(0);
    packet.extend_from_slice(&(total_len as u16).to_be_bytes());
    packet.extend_from_slice(&0x1234u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.push(64);
    packet.push(PROTO_TCP);
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&from.address.octets());
    packet.extend_from_slice(&to.address.octets());

    let header = ones_complement(&packet[ETH_LEN..ETH_LEN + IP_LEN]);
    packet[ETH_LEN + 10..ETH_LEN + 12].copy_from_slice(&header.to_be_bytes());

    packet.extend_from_slice(&from.port.to_be_bytes());
    packet.extend_from_slice(&to.port.to_be_bytes());
    packet.extend_from_slice(&1u32.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.push(0x50);
    packet.push(0x02);
    packet.extend_from_slice(&64240u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());

    let l4 = tcp_checksum(&packet);
    let offset = ETH_LEN + IP_LEN + 16;
    packet[offset..offset + 2].copy_from_slice(&l4.to_be_bytes());

    packet
}

pub fn arp_frame(from: [u8; 6], to: [u8; 6]) -> Vec<u8> {
    let mut frame = vec![0u8; 42];
    frame[0..6].copy_from_slice(&to);
    frame[6..12].copy_from_slice(&from);
    frame[12..14].copy_from_slice(&ETH_P_ARP.to_be_bytes());
    frame
}

pub fn tcp_checksum(packet: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + packet.len() - ETH_LEN - IP_LEN);
    pseudo.extend_from_slice(&packet[ETH_LEN + 12..ETH_LEN + 20]);
    pseudo.push(0);
    pseudo.push(PROTO_TCP);
    let tcp_len = (packet.len() - ETH_LEN - IP_LEN) as u16;
    pseudo.extend_from_slice(&tcp_len.to_be_bytes());
    pseudo.extend_from_slice(&packet[ETH_LEN + IP_LEN..]);
    ones_complement(&pseudo)
}

pub fn eth_dst(packet: &[u8]) -> [u8; 6] {
    packet[0..6]
        .try_into()
        .expect("frame has a destination mac")
}

pub fn eth_src(packet: &[u8]) -> [u8; 6] {
    packet[6..12].try_into().expect("frame has a source mac")
}

pub fn eth_proto(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[12], packet[13]])
}

pub fn ip_protocol(packet: &[u8]) -> u8 {
    packet[ETH_LEN + 9]
}

pub fn ip_total_len(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[ETH_LEN + 2], packet[ETH_LEN + 3]])
}

pub fn ip_src(packet: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(
        packet[ETH_LEN + 12],
        packet[ETH_LEN + 13],
        packet[ETH_LEN + 14],
        packet[ETH_LEN + 15],
    )
}

pub fn ip_dst(packet: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(
        packet[ETH_LEN + 16],
        packet[ETH_LEN + 17],
        packet[ETH_LEN + 18],
        packet[ETH_LEN + 19],
    )
}

pub fn tcp_src_port(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[ETH_LEN + IP_LEN], packet[ETH_LEN + IP_LEN + 1]])
}

pub fn tcp_dst_port(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[ETH_LEN + IP_LEN + 2], packet[ETH_LEN + IP_LEN + 3]])
}

pub fn ip_header_is_valid(packet: &[u8]) -> bool {
    ones_complement(&packet[ETH_LEN..ETH_LEN + IP_LEN]) == 0
}

pub fn tcp_checksum_is_valid(packet: &[u8]) -> bool {
    tcp_checksum(packet) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Endpoint {
        Endpoint {
            mac: [0x02, 0, 0, 0, 0, 0x10],
            address: "10.1.0.10".parse().unwrap(),
            port: 40000,
        }
    }

    fn vip() -> Endpoint {
        Endpoint {
            mac: [0x02, 0, 0, 0, 0, 0x01],
            address: "10.0.0.100".parse().unwrap(),
            port: 80,
        }
    }

    #[test]
    fn a_built_packet_has_valid_checksums() {
        let packet = tcp_syn(client(), vip());
        assert!(ip_header_is_valid(&packet));
        assert!(tcp_checksum_is_valid(&packet));
    }

    #[test]
    fn a_built_packet_has_the_expected_shape() {
        let packet = tcp_syn(client(), vip());
        assert_eq!(packet.len(), ETH_LEN + IP_LEN + TCP_LEN);
        assert_eq!(eth_proto(&packet), ETH_P_IP);
        assert_eq!(ip_protocol(&packet), PROTO_TCP);
        assert_eq!(ip_total_len(&packet) as usize, IP_LEN + TCP_LEN);
        assert_eq!(eth_src(&packet), client().mac);
        assert_eq!(eth_dst(&packet), vip().mac);
        assert_eq!(ip_src(&packet), client().address);
        assert_eq!(ip_dst(&packet), vip().address);
        assert_eq!(tcp_src_port(&packet), client().port);
        assert_eq!(tcp_dst_port(&packet), vip().port);
    }

    #[test]
    fn a_corrupted_header_fails_validation() {
        let mut packet = tcp_syn(client(), vip());
        packet[ETH_LEN + 12] ^= 0xff;
        assert!(!ip_header_is_valid(&packet));
    }
}
