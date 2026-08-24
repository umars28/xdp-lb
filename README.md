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

Jalan end-to-end di rig network namespace. Yang sudah terbukti berjalan, bukan sekadar terkompilasi:

```
$ make smoke
requests: 40
  be2: 23
  be1: 17
  failed: 0

$ sudo ./test/backend-down.sh be1     # backend mati
$ REQUESTS=20 make smoke
requests: 20
  be2: 20
  failed: 0                            # semua trafik pindah, nol koneksi gagal

$ sudo ./test/backend-up.sh be1        # backend kembali
$ REQUESTS=20 make smoke
requests: 20
  be2: 8
  be1: 12
```

Counter dari datapath saat 40 koneksi tersebut:

```
xdplb_packets_total{verdict="conntrack_miss"}  40     # tepat satu per koneksi baru
xdplb_packets_total{verdict="conntrack_hit"}  440     # paket lanjutan tidak menyentuh maglev
xdplb_packets_total{verdict="drop"}             0
```

Isi map `conntrack` 80 entry untuk 40 koneksi — dua per koneksi, satu untuk tiap arah.

Uji di kernel 6.8 (Ubuntu 24.04, aarch64), mode SKB di atas veth. Belum ada angka benchmark di NIC
fisik, jadi belum ada klaim performa apa pun. Lihat `docs/ROADMAP.md`.

## Requirement

XDP hanya ada di Linux. Kernel minimal 5.15, direkomendasikan 6.8+.

- `clang` + `llvm` (dengan BPF backend)
- `libbpf-dev` (header saja, tidak dipakai saat runtime)
- Rust stable 1.75+

Kalau development dari macOS, pakai VM Linux — lihat `docs/DEVELOPMENT.md`.

## Lisensi

Datapath BPF: GPL-2.0 (syarat kernel untuk akses helper tertentu). Control plane: MIT.
