use std::net::Ipv4Addr;

use crate::config::parse_mac;

const ARP_TABLE: &str = "/proc/net/arp";
const FLAG_COMPLETE: u32 = 0x2;

pub fn resolve(address: Ipv4Addr, interface: &str) -> Option<[u8; 6]> {
    let table = std::fs::read_to_string(ARP_TABLE).ok()?;
    lookup(&table, address, interface)
}

fn lookup(table: &str, address: Ipv4Addr, interface: &str) -> Option<[u8; 6]> {
    let wanted = address.to_string();

    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        if fields[0] != wanted || fields[5] != interface {
            continue;
        }

        let flags = fields[2]
            .strip_prefix("0x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .unwrap_or(0);
        if flags & FLAG_COMPLETE == 0 {
            continue;
        }

        return parse_mac(fields[3]).ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str =
        "IP address       HW type     Flags       HW address            Mask     Device
10.0.0.11        0x1         0x2         aa:bb:cc:00:00:11     *        eth0
10.0.0.12        0x1         0x0         00:00:00:00:00:00     *        eth0
10.0.0.13        0x1         0x2         aa:bb:cc:00:00:13     *        eth1
";

    #[test]
    fn resolves_complete_entry() {
        let mac = lookup(TABLE, "10.0.0.11".parse().unwrap(), "eth0");
        assert_eq!(mac, Some([0xaa, 0xbb, 0xcc, 0x00, 0x00, 0x11]));
    }

    #[test]
    fn skips_incomplete_entry() {
        assert_eq!(lookup(TABLE, "10.0.0.12".parse().unwrap(), "eth0"), None);
    }

    #[test]
    fn respects_interface() {
        assert_eq!(lookup(TABLE, "10.0.0.13".parse().unwrap(), "eth0"), None);
        assert!(lookup(TABLE, "10.0.0.13".parse().unwrap(), "eth1").is_some());
    }
}
