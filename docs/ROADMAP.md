# Roadmap

## Selesai

- Datapath XDP: parse Ethernet/IPv4/TCP/UDP dengan bounds check yang lolos verifier
- Conntrack LRU dua arah, jadi flow yang sudah jalan tetap ke backend yang sama
- Maglev consistent hashing dengan bobot, dihitung di control plane
- Active health check TCP, backend mati otomatis dikeluarkan dari tabel
- Resolusi MAC backend dari neighbour table
- Counter per-CPU dan per-backend, diekspos sebagai metrik Prometheus
- Rig test berbasis network namespace

## Berikutnya

**Benchmark yang bisa dipertanggungjawabkan.** Ini prioritas satu, karena tanpa angka, project ini
hanya kode. Butuh NIC fisik dengan XDP native support, pembanding (nginx stream dan IPVS di mesin
yang sama), dan tiga metrik: pps per core, p99 latency, dan CPU cycles per paket.

**Unit test datapath lewat `BPF_PROG_TEST_RUN`.** Bikin paket sintetis, jalankan program, periksa
verdict dan paket keluarannya — tanpa perlu topologi jaringan sama sekali. Ini yang membuat regresi
di logika parsing dan checksum ketangkap di CI, bukan di test manual.

**DSR mode dengan enkapsulasi IPIP.** Sekarang mode-nya NAT, jadi trafik balasan harus lewat load
balancer. Dengan DSR, backend menjawab langsung ke client, dan LB hanya memproses arah masuk. Ini
alasan utama orang memilih XDP untuk load balancing, dan lapangan kerjanya di sini.

**Graceful drain.** Backend ditandai draining: tidak menerima flow baru, tapi flow yang sudah ada di
conntrack tetap dilayani sampai selesai.

**Bobot dinamis dari Prometheus.** Control plane query metrik backend (utilisasi CPU, p99 latency,
in-flight request) dari Mimir, lalu menghitung ulang bobot maglev tiap interval. Ini yang membuat
"dynamic" di nama project jadi literal, bukan cuma berarti "backend bisa diubah saat runtime".

**Rate limiting di XDP.** Drop paket berlebih sebelum masuk network stack. Nilai praktisnya:
mitigasi SYN flood yang tidak menghabiskan CPU untuk paket yang akhirnya dibuang.

**CI multi-kernel.** Jalankan test di beberapa versi kernel supaya klaim kompatibilitas ada
buktinya, bukan asumsi.

## Sengaja tidak dikerjakan

**Kubernetes operator.** Menarik, tapi mayoritas kodenya boilerplate reconcile yang tidak
menunjukkan apa pun soal eBPF. Lebih baik jadi project terpisah.

**IPv6.** Menggandakan jalur parsing dan conntrack tanpa menambah hal baru yang dipelajari.

**L7 routing.** XDP tidak melihat stream TCP, hanya paket individual. Routing berbasis HTTP header
butuh terminasi koneksi, yang berarti keluar dari XDP.
