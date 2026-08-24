#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/in.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <linux/udp.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_endian.h>

#include "xdp_lb.h"

char LICENSE[] SEC("license") = "GPL";

struct {
	__uint(type, BPF_MAP_TYPE_HASH);
	__uint(max_entries, MAX_SERVICES);
	__type(key, struct service_key);
	__type(value, struct service_info);
} services SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, MAX_BACKENDS);
	__type(key, __u32);
	__type(value, struct backend);
} backends SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, MAX_SERVICES *MAGLEV_SIZE);
	__type(key, __u32);
	__type(value, __u32);
} maglev SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_LRU_HASH);
	__uint(max_entries, MAX_CONNTRACK);
	__type(key, struct flow_key);
	__type(value, struct nat_entry);
} conntrack SEC(".maps");

struct rate_bucket {
	__u64 tokens;
	__u64 next_refill_ns;
};

struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, 1);
	__type(key, __u32);
	__type(value, struct rate_config);
} rate_config SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_LRU_PERCPU_HASH);
	__uint(max_entries, MAX_RATE_BUCKETS);
	__type(key, __be32);
	__type(value, struct rate_bucket);
} rate_buckets SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
	__uint(max_entries, __STAT_MAX);
	__type(key, __u32);
	__type(value, struct stat_val);
} stats SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
	__uint(max_entries, MAX_BACKENDS);
	__type(key, __u32);
	__type(value, struct stat_val);
} backend_stats SEC(".maps");

static __always_inline void stat_bump(__u32 kind, __u64 bytes)
{
	struct stat_val *v = bpf_map_lookup_elem(&stats, &kind);

	if (v) {
		v->packets += 1;
		v->bytes += bytes;
	}
}

static __always_inline void backend_bump(__u32 idx, __u64 bytes)
{
	struct stat_val *v = bpf_map_lookup_elem(&backend_stats, &idx);

	if (v) {
		v->packets += 1;
		v->bytes += bytes;
	}
}

static __always_inline __u32 csum_add32(__u32 csum, __u32 addend)
{
	__u32 res = csum + addend;

	return res + (res < addend);
}

static __always_inline __u16 csum_fold(__u32 csum)
{
	csum = (csum & 0xffff) + (csum >> 16);
	csum = (csum & 0xffff) + (csum >> 16);
	return (__u16)~csum;
}

static __always_inline void csum_replace4(__u16 *sum, __be32 from, __be32 to)
{
	__u32 tmp = csum_add32(~((__u32)*sum), ~((__u32)from));

	*sum = csum_fold(csum_add32(tmp, (__u32)to));
}

static __always_inline __u16 csum_add16(__u16 csum, __u16 addend)
{
	__u16 res = csum + addend;

	return res + (res < addend);
}

static __always_inline void csum_replace2(__u16 *sum, __be16 from, __be16 to)
{
	__u16 tmp = csum_add16((__u16)~(*sum), (__u16)~from);

	*sum = ~csum_add16(tmp, (__u16)to);
}

static __always_inline __u32 rol32(__u32 word, __u32 shift)
{
	return (word << shift) | (word >> ((-shift) & 31));
}

static __always_inline __u32 flow_hash(const struct flow_key *k)
{
	__u32 a = (__u32)k->saddr + 0xdeadbeef;
	__u32 b = (__u32)k->daddr + 0xdeadbeef;
	__u32 c = (((__u32)k->sport << 16) | (__u32)k->dport) + k->proto + 0xdeadbeef;

	c ^= b;
	c -= rol32(b, 14);
	a ^= c;
	a -= rol32(c, 11);
	b ^= a;
	b -= rol32(a, 25);
	c ^= b;
	c -= rol32(b, 16);
	a ^= c;
	a -= rol32(c, 4);
	b ^= a;
	b -= rol32(a, 14);
	c ^= b;
	c -= rol32(b, 24);

	return c;
}

static __always_inline int new_flow_allowed(__be32 saddr, __u64 now)
{
	__u32 zero = 0;
	struct rate_config *cfg = bpf_map_lookup_elem(&rate_config, &zero);

	if (!cfg || !cfg->enabled || !cfg->interval_ns || !cfg->burst)
		return 1;

	struct rate_bucket *bucket = bpf_map_lookup_elem(&rate_buckets, &saddr);

	if (!bucket) {
		struct rate_bucket fresh = {};

		fresh.tokens = cfg->burst - 1;
		fresh.next_refill_ns = now + cfg->interval_ns;
		bpf_map_update_elem(&rate_buckets, &saddr, &fresh, BPF_ANY);
		return 1;
	}

	if (now >= bucket->next_refill_ns) {
		__u64 gained = 1 + (now - bucket->next_refill_ns) / cfg->interval_ns;

		bucket->tokens += gained;
		if (bucket->tokens > cfg->burst)
			bucket->tokens = cfg->burst;
		bucket->next_refill_ns = now + cfg->interval_ns;
	}

	if (!bucket->tokens)
		return 0;

	bucket->tokens -= 1;

	return 1;
}

static __always_inline __u16 header_csum(struct iphdr *iph)
{
	__u16 *word = (__u16 *)iph;
	__u32 sum = 0;
	int i;

	iph->check = 0;

#pragma clang loop unroll(full)
	for (i = 0; i < (int)(sizeof(*iph) / 2); i++)
		sum += word[i];

	return csum_fold(sum);
}

static __always_inline int encap_dsr(struct xdp_md *ctx, struct nat_entry *nat, __u64 pkt_len)
{
	void *data = (void *)(long)ctx->data;
	void *data_end = (void *)(long)ctx->data_end;
	struct ethhdr *eth = data;

	if ((void *)(eth + 1) > data_end)
		return XDP_DROP;

	struct iphdr *inner = (void *)(eth + 1);

	if ((void *)(inner + 1) > data_end)
		return XDP_DROP;

	struct ethhdr saved;
	__u16 inner_len = bpf_ntohs(inner->tot_len);

	__builtin_memcpy(&saved, eth, sizeof(saved));

	if (bpf_xdp_adjust_head(ctx, 0 - (int)sizeof(struct iphdr))) {
		stat_bump(STAT_NO_HEADROOM, pkt_len);
		return XDP_DROP;
	}

	data = (void *)(long)ctx->data;
	data_end = (void *)(long)ctx->data_end;

	struct ethhdr *outer_eth = data;

	if ((void *)(outer_eth + 1) > data_end)
		return XDP_DROP;

	struct iphdr *outer = (void *)(outer_eth + 1);

	if ((void *)(outer + 1) > data_end)
		return XDP_DROP;

	__builtin_memcpy(outer_eth->h_source, saved.h_dest, ETH_ALEN);
	__builtin_memcpy(outer_eth->h_dest, nat->dmac, ETH_ALEN);
	outer_eth->h_proto = bpf_htons(ETH_P_IP);

	outer->version = 4;
	outer->ihl = 5;
	outer->tos = 0;
	outer->tot_len = bpf_htons(inner_len + (__u16)sizeof(struct iphdr));
	outer->id = 0;
	outer->frag_off = 0;
	outer->ttl = 64;
	outer->protocol = IPPROTO_IPIP;
	outer->saddr = nat->outer_saddr;
	outer->daddr = nat->addr;
	outer->check = header_csum(outer);

	backend_bump(nat->backend_idx, pkt_len);
	stat_bump(STAT_TX, pkt_len);

	return XDP_TX;
}

static __always_inline int apply_nat(struct ethhdr *eth, struct iphdr *iph,
				     struct l4ports *ports, __u16 *l4_csum,
				     struct nat_entry *nat, __u64 pkt_len)
{
	__be32 old_ip;
	__be16 old_port;
	__u8 lb_mac[ETH_ALEN];

	if (nat->flags & NAT_DIR_FWD) {
		old_ip = iph->daddr;
		old_port = ports->dest;
		iph->daddr = nat->addr;
		ports->dest = nat->port;
	} else {
		old_ip = iph->saddr;
		old_port = ports->source;
		iph->saddr = nat->addr;
		ports->source = nat->port;
	}

	csum_replace4(&iph->check, old_ip, nat->addr);
	if (l4_csum) {
		csum_replace4(l4_csum, old_ip, nat->addr);
		csum_replace2(l4_csum, old_port, nat->port);
	}

	__builtin_memcpy(lb_mac, eth->h_dest, ETH_ALEN);
	__builtin_memcpy(eth->h_dest, nat->dmac, ETH_ALEN);
	__builtin_memcpy(eth->h_source, lb_mac, ETH_ALEN);

	backend_bump(nat->backend_idx, pkt_len);
	stat_bump(STAT_TX, pkt_len);

	return XDP_TX;
}

SEC("xdp")
int xdp_lb(struct xdp_md *ctx)
{
	void *data = (void *)(long)ctx->data;
	void *data_end = (void *)(long)ctx->data_end;
	__u64 pkt_len = data_end - data;
	__u16 *l4_csum = NULL;

	stat_bump(STAT_RX, pkt_len);

	struct ethhdr *eth = data;

	if ((void *)(eth + 1) > data_end)
		return XDP_PASS;
	if (eth->h_proto != bpf_htons(ETH_P_IP))
		return XDP_PASS;

	struct iphdr *iph = (void *)(eth + 1);

	if ((void *)(iph + 1) > data_end)
		return XDP_PASS;
	if (iph->ihl != 5)
		return XDP_PASS;
	if (iph->protocol != IPPROTO_TCP && iph->protocol != IPPROTO_UDP)
		return XDP_PASS;

	void *l4 = (void *)(iph + 1);
	struct l4ports *ports = l4;

	if ((void *)(ports + 1) > data_end)
		return XDP_PASS;

	if (iph->protocol == IPPROTO_TCP) {
		struct tcphdr *th = l4;

		if ((void *)(th + 1) > data_end)
			return XDP_PASS;
		l4_csum = &th->check;
	} else {
		struct udphdr *uh = l4;

		if ((void *)(uh + 1) > data_end)
			return XDP_PASS;
		if (uh->check)
			l4_csum = &uh->check;
	}

	struct flow_key fk = {};

	fk.saddr = iph->saddr;
	fk.daddr = iph->daddr;
	fk.sport = ports->source;
	fk.dport = ports->dest;
	fk.proto = iph->protocol;

	struct nat_entry *hit = bpf_map_lookup_elem(&conntrack, &fk);

	if (hit) {
		hit->last_seen = bpf_ktime_get_ns();
		stat_bump(STAT_CT_HIT, pkt_len);
		if (hit->flags & NAT_DIR_DSR)
			return encap_dsr(ctx, hit, pkt_len);
		return apply_nat(eth, iph, ports, l4_csum, hit, pkt_len);
	}

	struct service_key sk = {};

	sk.vip = iph->daddr;
	sk.port = ports->dest;
	sk.proto = iph->protocol;

	struct service_info *svc = bpf_map_lookup_elem(&services, &sk);

	if (!svc) {
		stat_bump(STAT_PASS, pkt_len);
		return XDP_PASS;
	}

	stat_bump(STAT_CT_MISS, pkt_len);

	__u64 now = bpf_ktime_get_ns();

	if (!new_flow_allowed(iph->saddr, now)) {
		stat_bump(STAT_RATE_LIMITED, pkt_len);
		return XDP_DROP;
	}

	__u32 slot = svc->svc_id * MAGLEV_SIZE + (flow_hash(&fk) % MAGLEV_SIZE);
	__u32 *chosen = bpf_map_lookup_elem(&maglev, &slot);

	if (!chosen || *chosen >= MAX_BACKENDS) {
		stat_bump(STAT_NO_BACKEND, pkt_len);
		return XDP_DROP;
	}

	__u32 idx = *chosen;
	struct backend *be = bpf_map_lookup_elem(&backends, &idx);

	if (!be || !be->addr || !(be->flags & BACKEND_ACTIVE)) {
		stat_bump(STAT_NO_BACKEND, pkt_len);
		return XDP_DROP;
	}

	struct nat_entry fwd = {};

	fwd.addr = be->addr;
	fwd.port = be->port;
	__builtin_memcpy(fwd.dmac, be->mac, ETH_ALEN);
	fwd.backend_idx = idx;
	fwd.last_seen = now;

	if (svc->mode == MODE_DSR) {
		fwd.flags = NAT_DIR_DSR;
		fwd.outer_saddr = svc->dsr_source;
		bpf_map_update_elem(&conntrack, &fk, &fwd, BPF_ANY);
		return encap_dsr(ctx, &fwd, pkt_len);
	}

	fwd.flags = NAT_DIR_FWD;

	struct flow_key rk = {};

	rk.saddr = be->addr;
	rk.daddr = iph->saddr;
	rk.sport = be->port;
	rk.dport = ports->source;
	rk.proto = iph->protocol;

	struct nat_entry rev = {};

	rev.addr = iph->daddr;
	rev.port = ports->dest;
	rev.flags = NAT_DIR_REV;
	__builtin_memcpy(rev.dmac, eth->h_source, ETH_ALEN);
	rev.backend_idx = idx;
	rev.last_seen = now;

	bpf_map_update_elem(&conntrack, &fk, &fwd, BPF_ANY);
	bpf_map_update_elem(&conntrack, &rk, &rev, BPF_ANY);

	return apply_nat(eth, iph, ports, l4_csum, &fwd, pkt_len);
}
