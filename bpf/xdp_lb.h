#ifndef __XDP_LB_H
#define __XDP_LB_H

#define MAX_BACKENDS 4096
#define MAX_SERVICES 64
#define MAGLEV_SIZE 4099
#define MAX_CONNTRACK 1048576

#define BACKEND_ACTIVE (1 << 0)
#define BACKEND_DRAINING (1 << 1)

#define NAT_DIR_FWD (1 << 0)
#define NAT_DIR_REV (1 << 1)
#define NAT_DIR_DSR (1 << 2)

#define MODE_NAT 0
#define MODE_DSR 1

#define MAX_RATE_BUCKETS 1048576

enum stat_kind {
	STAT_RX = 0,
	STAT_TX,
	STAT_PASS,
	STAT_DROP,
	STAT_CT_HIT,
	STAT_CT_MISS,
	STAT_NO_BACKEND,
	STAT_RATE_LIMITED,
	STAT_NO_HEADROOM,
	__STAT_MAX,
};

struct rate_config {
	__u64 interval_ns;
	__u64 burst;
	__u8 enabled;
	__u8 pad[7];
};

struct service_key {
	__be32 vip;
	__be16 port;
	__u8 proto;
	__u8 pad;
};

struct service_info {
	__u32 svc_id;
	__be32 dsr_source;
	__u8 mode;
	__u8 pad[3];
};

struct backend {
	__be32 addr;
	__be16 port;
	__u16 flags;
	__u8 mac[6];
	__u8 pad[2];
};

struct flow_key {
	__be32 saddr;
	__be32 daddr;
	__be16 sport;
	__be16 dport;
	__u8 proto;
	__u8 pad[3];
};

struct nat_entry {
	__be32 addr;
	__be16 port;
	__u16 flags;
	__u8 dmac[6];
	__u8 pad[2];
	__u32 backend_idx;
	__be32 outer_saddr;
	__u64 last_seen;
};

struct stat_val {
	__u64 packets;
	__u64 bytes;
};

struct l4ports {
	__be16 source;
	__be16 dest;
};

#endif
