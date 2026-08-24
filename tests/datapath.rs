use std::{
    net::Ipv4Addr,
    os::fd::{AsFd, OwnedFd},
};

use aya::{programs::Xdp, Ebpf};
use xdp_lb::{
    dataplane::DataPlane,
    progtest::{self, XDP_DROP, XDP_PASS, XDP_TX},
    types::{
        be16, be32, Backend, ServiceInfo, ServiceKey, BACKEND_ACTIVE, MAGLEV_SIZE, MODE_NAT,
        NO_BACKEND,
    },
};

const CLIENT_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x10];
const LB_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
const BACKEND_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x11];

const CLIENT_IP: &str = "10.1.0.10";
const VIP: &str = "10.0.0.100";
const BACKEND_IP: &str = "10.0.0.11";

const CLIENT_PORT: u16 = 40000;
const VIP_PORT: u16 = 80;
const BACKEND_PORT: u16 = 8080;

const PROTO_TCP: u8 = 6;
const ETH_LEN: usize = 14;
const IP_LEN: usize = 20;

struct Harness {
    _ebpf: Ebpf,
    program: OwnedFd,
    plane: DataPlane,
}

impl Harness {
    fn load() -> Self {
        assert!(
            unsafe { libc::geteuid() } == 0,
            "datapath tests drive BPF_PROG_TEST_RUN and need root: run `make test-datapath`"
        );

        let mut ebpf = Ebpf::load(xdp_lb::object::bytes()).expect("object must load");

        let program: &mut Xdp = ebpf
            .program_mut("xdp_lb")
            .expect("program xdp_lb must exist")
            .try_into()
            .expect("program must be an xdp program");
        program.load().expect("verifier must accept the program");

        let fd = program
            .fd()
            .expect("program must expose a fd")
            .as_fd()
            .try_clone_to_owned()
            .expect("fd must be cloneable");

        let plane = DataPlane::attach_maps(&mut ebpf).expect("maps must be present");

        Self {
            _ebpf: ebpf,
            program: fd,
            plane,
        }
    }

    fn publish_service(&mut self) {
        self.plane
            .put_service(
                ServiceKey {
                    vip: be32(VIP.parse().unwrap()),
                    port: be16(VIP_PORT),
                    proto: PROTO_TCP,
                    pad: 0,
                },
                ServiceInfo {
                    svc_id: 0,
                    mode: MODE_NAT,
                    pad: [0; 3],
                },
            )
            .expect("service must be writable");
    }

    fn publish_backend(&mut self, flags: u16) {
        self.plane
            .put_backend(
                0,
                Backend {
                    addr: be32(BACKEND_IP.parse().unwrap()),
                    port: be16(BACKEND_PORT),
                    flags,
                    mac: BACKEND_MAC,
                    pad: [0; 2],
                },
            )
            .expect("backend must be writable");
    }

    fn point_every_slot_at_backend_zero(&mut self) {
        self.plane
            .put_maglev_table(0, &vec![0u32; MAGLEV_SIZE as usize])
            .expect("maglev table must be writable");
    }

    fn point_every_slot_at_nothing(&mut self) {
        self.plane
            .put_maglev_table(0, &vec![NO_BACKEND; MAGLEV_SIZE as usize])
            .expect("maglev table must be writable");
    }

    fn ready(&mut self) {
        self.publish_service();
        self.publish_backend(BACKEND_ACTIVE);
        self.point_every_slot_at_backend_zero();
    }

    fn run(&self, packet: &[u8]) -> progtest::Outcome {
        progtest::run(self.program.as_fd(), packet, 1).expect("BPF_PROG_TEST_RUN must succeed")
    }

    fn stat(&self, name: &str) -> u64 {
        self.plane
            .global_stats()
            .expect("stats must be readable")
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.packets)
            .unwrap_or_else(|| panic!("no stat named {name}"))
    }
}

fn ones_complement(bytes: &[u8]) -> u16 {
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

fn tcp_packet(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
) -> Vec<u8> {
    let payload_len = 20usize;
    let total_len = IP_LEN + payload_len;

    let mut packet = Vec::with_capacity(ETH_LEN + total_len);
    packet.extend_from_slice(&dst_mac);
    packet.extend_from_slice(&src_mac);
    packet.extend_from_slice(&0x0800u16.to_be_bytes());

    packet.push(0x45);
    packet.push(0);
    packet.extend_from_slice(&(total_len as u16).to_be_bytes());
    packet.extend_from_slice(&0x1234u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.push(64);
    packet.push(PROTO_TCP);
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&src_ip.octets());
    packet.extend_from_slice(&dst_ip.octets());

    let ip_csum = ones_complement(&packet[ETH_LEN..ETH_LEN + IP_LEN]);
    packet[ETH_LEN + 10..ETH_LEN + 12].copy_from_slice(&ip_csum.to_be_bytes());

    packet.extend_from_slice(&src_port.to_be_bytes());
    packet.extend_from_slice(&dst_port.to_be_bytes());
    packet.extend_from_slice(&1u32.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.push(0x50);
    packet.push(0x02);
    packet.extend_from_slice(&64240u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());

    let tcp_csum = tcp_checksum(&packet);
    let offset = ETH_LEN + IP_LEN + 16;
    packet[offset..offset + 2].copy_from_slice(&tcp_csum.to_be_bytes());

    packet
}

fn tcp_checksum(packet: &[u8]) -> u16 {
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&packet[ETH_LEN + 12..ETH_LEN + 20]);
    pseudo.push(0);
    pseudo.push(PROTO_TCP);
    let tcp_len = (packet.len() - ETH_LEN - IP_LEN) as u16;
    pseudo.extend_from_slice(&tcp_len.to_be_bytes());
    pseudo.extend_from_slice(&packet[ETH_LEN + IP_LEN..]);
    ones_complement(&pseudo)
}

fn eth_dst(packet: &[u8]) -> [u8; 6] {
    packet[0..6].try_into().unwrap()
}

fn eth_src(packet: &[u8]) -> [u8; 6] {
    packet[6..12].try_into().unwrap()
}

fn ip_src(packet: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(
        packet[ETH_LEN + 12],
        packet[ETH_LEN + 13],
        packet[ETH_LEN + 14],
        packet[ETH_LEN + 15],
    )
}

fn ip_dst(packet: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(
        packet[ETH_LEN + 16],
        packet[ETH_LEN + 17],
        packet[ETH_LEN + 18],
        packet[ETH_LEN + 19],
    )
}

fn tcp_src_port(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[ETH_LEN + IP_LEN], packet[ETH_LEN + IP_LEN + 1]])
}

fn tcp_dst_port(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[ETH_LEN + IP_LEN + 2], packet[ETH_LEN + IP_LEN + 3]])
}

fn assert_checksums_valid(packet: &[u8]) {
    assert_eq!(
        ones_complement(&packet[ETH_LEN..ETH_LEN + IP_LEN]),
        0,
        "IPv4 header checksum is wrong after rewrite"
    );
    assert_eq!(
        tcp_checksum(packet),
        0,
        "TCP checksum is wrong after rewrite"
    );
}

fn forward_packet() -> Vec<u8> {
    tcp_packet(
        CLIENT_MAC,
        LB_MAC,
        CLIENT_IP.parse().unwrap(),
        VIP.parse().unwrap(),
        CLIENT_PORT,
        VIP_PORT,
    )
}

fn reply_packet() -> Vec<u8> {
    tcp_packet(
        BACKEND_MAC,
        LB_MAC,
        BACKEND_IP.parse().unwrap(),
        CLIENT_IP.parse().unwrap(),
        BACKEND_PORT,
        CLIENT_PORT,
    )
}

#[test]
#[ignore = "needs root"]
fn packet_builder_produces_valid_checksums() {
    assert_checksums_valid(&forward_packet());
    assert_checksums_valid(&reply_packet());
}

#[test]
#[ignore = "needs root"]
fn non_ipv4_frame_is_passed_to_the_stack() {
    let harness = Harness::load();

    let mut arp = vec![0u8; 42];
    arp[0..6].copy_from_slice(&LB_MAC);
    arp[6..12].copy_from_slice(&CLIENT_MAC);
    arp[12..14].copy_from_slice(&0x0806u16.to_be_bytes());

    let outcome = harness.run(&arp);
    assert_eq!(
        outcome.verdict,
        XDP_PASS,
        "ARP must reach the stack, got {}",
        outcome.verdict_name()
    );
}

#[test]
#[ignore = "needs root"]
fn truncated_ip_header_is_passed_to_the_stack() {
    let harness = Harness::load();

    let mut runt = forward_packet();
    runt.truncate(ETH_LEN + 10);

    let outcome = harness.run(&runt);
    assert_eq!(
        outcome.verdict,
        XDP_PASS,
        "a truncated header must not be parsed, got {}",
        outcome.verdict_name()
    );
}

#[test]
#[ignore = "needs root"]
fn traffic_for_an_unknown_destination_is_passed_to_the_stack() {
    let mut harness = Harness::load();
    harness.ready();

    let stray = tcp_packet(
        CLIENT_MAC,
        LB_MAC,
        CLIENT_IP.parse().unwrap(),
        "10.0.0.99".parse().unwrap(),
        CLIENT_PORT,
        VIP_PORT,
    );

    let outcome = harness.run(&stray);
    assert_eq!(
        outcome.verdict,
        XDP_PASS,
        "only configured VIPs may be intercepted, got {}",
        outcome.verdict_name()
    );
}

#[test]
#[ignore = "needs root"]
fn vip_traffic_is_rewritten_towards_the_backend() {
    let mut harness = Harness::load();
    harness.ready();

    let outcome = harness.run(&forward_packet());
    assert_eq!(
        outcome.verdict,
        XDP_TX,
        "expected the packet to be transmitted, got {}",
        outcome.verdict_name()
    );

    let out = &outcome.packet;
    assert_eq!(ip_dst(out), BACKEND_IP.parse::<Ipv4Addr>().unwrap());
    assert_eq!(tcp_dst_port(out), BACKEND_PORT);
    assert_eq!(
        ip_src(out),
        CLIENT_IP.parse::<Ipv4Addr>().unwrap(),
        "the client address must survive so the backend can reply"
    );
    assert_eq!(tcp_src_port(out), CLIENT_PORT);
    assert_eq!(eth_dst(out), BACKEND_MAC);
    assert_eq!(
        eth_src(out),
        LB_MAC,
        "the load balancer must become the source of the frame"
    );
    assert_checksums_valid(out);
}

#[test]
#[ignore = "needs root"]
fn backend_reply_is_rewritten_back_to_the_vip() {
    let mut harness = Harness::load();
    harness.ready();

    harness.run(&forward_packet());

    let outcome = harness.run(&reply_packet());
    assert_eq!(
        outcome.verdict,
        XDP_TX,
        "the reply must be transmitted to the client, got {}",
        outcome.verdict_name()
    );

    let out = &outcome.packet;
    assert_eq!(
        ip_src(out),
        VIP.parse::<Ipv4Addr>().unwrap(),
        "the client opened the connection to the VIP and must see the VIP reply"
    );
    assert_eq!(tcp_src_port(out), VIP_PORT);
    assert_eq!(ip_dst(out), CLIENT_IP.parse::<Ipv4Addr>().unwrap());
    assert_eq!(tcp_dst_port(out), CLIENT_PORT);
    assert_eq!(eth_dst(out), CLIENT_MAC);
    assert_eq!(eth_src(out), LB_MAC);
    assert_checksums_valid(out);
}

#[test]
#[ignore = "needs root"]
fn established_flows_take_the_conntrack_path() {
    let mut harness = Harness::load();
    harness.ready();

    let packet = forward_packet();

    harness.run(&packet);
    assert_eq!(harness.stat("conntrack_miss"), 1);
    assert_eq!(harness.stat("conntrack_hit"), 0);

    harness.run(&packet);
    harness.run(&packet);

    assert_eq!(
        harness.stat("conntrack_miss"),
        1,
        "a known flow must not be hashed again"
    );
    assert_eq!(harness.stat("conntrack_hit"), 2);
}

#[test]
#[ignore = "needs root"]
fn vip_traffic_is_dropped_when_no_backend_is_active() {
    let mut harness = Harness::load();
    harness.publish_service();
    harness.publish_backend(0);
    harness.point_every_slot_at_backend_zero();

    let outcome = harness.run(&forward_packet());
    assert_eq!(
        outcome.verdict,
        XDP_DROP,
        "an inactive backend must not receive traffic, got {}",
        outcome.verdict_name()
    );
    assert_eq!(harness.stat("no_backend"), 1);
}

#[test]
#[ignore = "needs root"]
fn vip_traffic_is_dropped_when_the_table_is_empty() {
    let mut harness = Harness::load();
    harness.publish_service();
    harness.publish_backend(BACKEND_ACTIVE);
    harness.point_every_slot_at_nothing();

    let outcome = harness.run(&forward_packet());
    assert_eq!(
        outcome.verdict,
        XDP_DROP,
        "an empty maglev table must not select a backend, got {}",
        outcome.verdict_name()
    );
    assert_eq!(harness.stat("no_backend"), 1);
}
