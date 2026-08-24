# xdp-lb

Layer-4 load balancer yang jalan di XDP hook — packet processing di kernel, sebelum masuk network
stack. Datapath ditulis di C, control plane di Rust dengan [aya](https://aya-rs.dev) sebagai loader.

## Kenapa XDP

Load balancer L4 konvensional (nginx stream, HAProxy) memproses paket setelah melewati seluruh
network stack kernel — alokasi `sk_buff`, conntrack, routing lookup. XDP hook berjalan di titik
paling awal yang bisa dijangkau eBPF, langsung setelah driver menerima paket, tanpa `sk_buff`.
Konsekuensinya: throughput per core jauh lebih tinggi dan latensi tail lebih rendah, dengan harga
programming model yang jauh lebih terbatas.

## Arsitektur

```
                      ┌─────────────────────────────┐
                      │   control plane (Rust)      │
                      │                             │
                      │  health check ──┐           │
                      │  maglev table ──┤           │
                      │  ARP resolve  ──┤           │
                      │  /metrics     ←─┘           │
                      └──────────┬──────────────────┘
                                 │ BPF maps
                                 ▼
   NIC ──▶ XDP hook ──▶ ┌──────────────────────┐ ──▶ XDP_TX ──▶ backend
                        │  datapath (C)        │
                        │  parse → conntrack   │ ──▶ XDP_PASS ──▶ kernel stack
                        │  → maglev → NAT      │
                        └──────────────────────┘ ──▶ XDP_DROP
```

Datapath tidak pernah mengambil keputusan yang butuh I/O. Semua keputusan yang butuh state
eksternal — backend mana yang sehat, bobot berapa, MAC address apa — dihitung di control plane dan
ditulis ke BPF map. Datapath hanya membaca map.

## Status

Sedang dikerjakan. Lihat `docs/ROADMAP.md`.

## Requirement

XDP hanya ada di Linux. Kernel minimal 5.15, direkomendasikan 6.8+.

- `clang` + `llvm` (dengan BPF backend)
- `libbpf-dev` (header saja, tidak dipakai saat runtime)
- Rust stable 1.75+

Kalau development dari macOS, pakai VM Linux — lihat `docs/DEVELOPMENT.md`.

## Lisensi

Datapath BPF: GPL-2.0 (syarat kernel untuk akses helper tertentu). Control plane: MIT.
