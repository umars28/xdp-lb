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
                      │  prom weights ──┤           │
                      │  drain API    ──┤           │
                      │  /metrics     ←─┘           │
                      └──────────┬──────────────────┘
                                 │ BPF maps
                                 ▼
   NIC ──▶ XDP hook ──▶ ┌──────────────────────┐ ──▶ XDP_TX ──▶ backend
                        │  datapath (C)        │
                        │  parse → conntrack   │ ──▶ XDP_PASS ──▶ kernel stack
                        │  → rate limit        │
                        │  → maglev → NAT      │ ──▶ XDP_DROP
                        └──────────────────────┘
```

Datapath tidak pernah mengambil keputusan yang butuh I/O. Semua keputusan yang butuh state
eksternal — backend mana yang sehat, bobot berapa, MAC address apa — dihitung di control plane dan
ditulis ke BPF map. Datapath hanya membaca map.

Konsekuensi desain ini: serumit apa pun logika di control plane, biaya per paket tetap sama. Bobot
yang mengikuti utilisasi CPU backend secara real-time tetap berujung pada satu lookup array di
datapath.

## Fitur

| | |
| --- | --- |
| Pemilihan backend | Maglev consistent hashing dengan bobot |
| Persistensi flow | Conntrack LRU dua arah, satu juta entry |
| Mode forwarding | NAT (DNAT masuk, SNAT balik) atau DSR (enkapsulasi IPIP), per service |
| Health check | TCP connect aktif, paralel, dengan timeout |
| Bobot dinamis | Query PromQL ke Prometheus/Mimir, mode `proportional` atau `inverse` |
| Graceful drain | `POST /drain` — flow baru berhenti, flow lama diselesaikan |
| Rate limiting | Token bucket per alamat sumber, hanya pada pembentukan flow baru |
| Observability | Metrik Prometheus dari counter per-CPU dan per-backend |

## Status

Jalan end-to-end. Yang berikut ini terukur, bukan diklaim:

```
$ make smoke
requests: 40
  be2: 23
  be1: 17
  failed: 0
```

**Failover** — backend mati, trafik pindah, nol koneksi gagal:

```
$ sudo ./test/backend-down.sh be1
$ REQUESTS=20 make smoke
requests: 20
  be2: 20
  failed: 0
```

**Graceful drain** — backend tetap sehat, tapi tidak lagi menerima flow baru:

```
$ curl -X POST "localhost:9500/drain?backend=10.0.0.11:8080"
10.0.0.11:8080 draining
$ REQUESTS=20 make smoke
requests: 20
  be2: 20
  failed: 0
$ curl -s localhost:9500/metrics | grep 10.0.0.11
xdplb_backend_up{...}       1        <- masih sehat
xdplb_backend_draining{...} 1        <- tapi keluar dari rotasi
```

**Bobot mengikuti metrik** — backend kedua diturunkan ke kapasitas 25%:

```
xdplb_backend_weight{...10.0.0.11:8080"} 16
xdplb_backend_weight{...10.0.0.12:8080"} 4

requests: 60
  be1: 48
  be2: 12        <- tepat 4:1, sama dengan rasio bobotnya
  failed: 0
```

**Prometheus mati** — bobot terakhir dipertahankan, trafik tidak terganggu:

```
xdplb_weight_refresh_failed_total 1
requests: 20 ... failed: 0
```

**DSR memotong beban LB** — 30 request yang sama, service yang sama, hanya mode forwarding berbeda:

| | NAT | DSR | |
| --- | --- | --- | --- |
| Paket diproses LB | 412 | 238 | -42% |
| Byte diproses LB | 35.494 | 18.674 | -47% |

Dan itu meskipun DSR *menambah* 20 byte enkapsulasi per paket arah masuk. Bahwa balasan benar-benar
tidak lewat LB diverifikasi tiga cara terpisah — lihat `docs/FORWARDING.md`.

**Biaya datapath per paket** — median 5 run, dua arsitektur (`make bench`):

| | aarch64 | x86_64 |
| --- | --- | --- |
| flow berjalan (NAT) | 68 ns | 160 ns |
| flow baru (NAT) | 413 ns | 734 ns |
| flow baru (DSR) | 284 ns | 509 ns |
| selisih DSR vs NAT untuk flow baru | **−31%** | **−31%** |

Angka −31% yang identik di dua CPU yang sangat berbeda bukan kebetulan pengukuran: NAT menulis dua
entry conntrack, DSR hanya satu. Detail metode dan batasannya di `docs/BENCHMARK.md`.

Counter datapath saat 40 koneksi (mode NAT):

```
xdplb_packets_total{verdict="conntrack_miss"}  40     # tepat satu per koneksi baru
xdplb_packets_total{verdict="conntrack_hit"}  440     # paket lanjutan tidak menyentuh maglev
xdplb_packets_total{verdict="drop"}             0
```

Isi map `conntrack` 80 entry untuk 40 koneksi — dua per koneksi, satu tiap arah.

## Test

Dua lapisan, karena keduanya bisa hijau sementara integrasinya rusak.

```
make test            # 51 test control plane, tanpa root
make test-datapath   # 16 test datapath lewat BPF_PROG_TEST_RUN, butuh root
```

Test datapath menjalankan program XDP dengan paket sintetis, tanpa NIC, bridge, atau routing sama
sekali. Dua di antaranya memverifikasi checksum IPv4 dan TCP dengan menghitung ulang dari nol atas
paket keluaran — jadi kode checksum inkremental benar-benar diuji, bukan diasumsikan benar karena
`curl`-nya berhasil.

CI menjalankan kedua lapisan itu plus trafik nyata lewat rig network namespace pada setiap push.

## Batasan yang perlu diketahui

**Belum ada perbandingan throughput dengan nginx atau IPVS.** Yang ada baru biaya CPU per paket di
dalam datapath (`make bench`, lihat `docs/BENCHMARK.md`) — diukur tanpa NIC, tanpa driver, tanpa
kontensi. Itu batas atas yang dipaksakan datapath pada satu core, bukan throughput sistem.
Perbandingan head-to-head butuh dua mesin bare-metal dan masih ada di roadmap. Semua pengujian
fungsional juga berjalan di mode SKB di atas veth, yang membuang keuntungan performa utama XDP.

**Mode NAT menuntut LB di jalur balik.** Balasan backend harus lewat load balancer agar source IP
bisa dikembalikan ke VIP. Kalau itu tidak bisa dijamin di topologimu, pakai `forwarding: dsr`.

**DSR tidak memeriksa MTU.** Enkapsulasi menambah 20 byte; paket yang sudah sebesar MTU akan dibuang
driver tanpa jejak. Menambahkan pemeriksaan berarti menanamkan asumsi angka MTU ke datapath, jadi
untuk sekarang ini batasan yang didokumentasikan, bukan asumsi tersembunyi.

**Rate limiting bersifat aproksimatif.** Bucket-nya per-CPU, jadi batas agregatnya mendekati angka
yang dikonfigurasi ketika trafik menyebar rata antar CPU, dan lebih ketat ketika menumpuk di satu
CPU. Ini pertukaran yang disengaja; alasannya ada di `docs/NOTES.md`.

**Diuji di dua arsitektur, satu versi kernel.** aarch64 kernel 6.8.0-137 dan x86_64 kernel 6.8.0-49
— 52 test control plane dan 17 test datapath lolos di keduanya. Klaim kompatibilitas ke kernel yang
lebih lama belum ada buktinya sampai CI multi-kernel jalan.

## Requirement

XDP hanya ada di Linux. Kernel minimal 5.15, direkomendasikan 6.8+.

- `clang` + `llvm` (dengan BPF backend)
- `libbpf-dev` (header saja, tidak dipakai saat runtime)
- Rust stable 1.75+

Kalau development dari macOS, pakai VM Linux — lihat `docs/DEVELOPMENT.md`.

## Dokumentasi

| | |
| --- | --- |
| `docs/DEVELOPMENT.md` | Setup VM, rig test, dan urutan debugging |
| `docs/FORWARDING.md` | NAT vs DSR: cara kerja, syarat backend, dan angka perbandingannya |
| `docs/BENCHMARK.md` | Biaya per paket tiap jalur, dan kenapa metode pertama saya salah |
| `docs/WEIGHTING.md` | Bobot dinamis: mode, pencocokan metrik, perilaku saat metrik hilang |
| `docs/NOTES.md` | Bug yang menghabiskan waktu paling banyak dan penjelasannya |
| `docs/ROADMAP.md` | Yang belum dikerjakan dan yang sengaja tidak dikerjakan |

## Lisensi

Datapath BPF: GPL-2.0 (syarat kernel untuk akses helper tertentu). Control plane: MIT.
