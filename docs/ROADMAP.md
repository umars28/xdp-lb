# Roadmap

## Selesai

- Datapath XDP: parse Ethernet/IPv4/TCP/UDP dengan bounds check yang lolos verifier
- Conntrack LRU dua arah, jadi flow yang sudah jalan tetap ke backend yang sama
- Maglev consistent hashing dengan bobot, dihitung di control plane
- Active health check TCP, backend mati otomatis dikeluarkan dari tabel
- Resolusi MAC backend dari neighbour table
- Counter per-CPU dan per-backend, diekspos sebagai metrik Prometheus
- Rig test berbasis network namespace, idempoten
- Unit test datapath lewat `BPF_PROG_TEST_RUN` — 14 test, tanpa perlu topologi jaringan
- Graceful drain lewat admin endpoint, dengan jaminan flow lama tetap dilayani
- Bobot dinamis dari Prometheus, dengan fail-safe saat metrik hilang
- Rate limiting flow baru per alamat sumber, token bucket per-CPU di XDP
- DSR mode dengan enkapsulasi IPIP, terukur memotong 42% paket dan 47% byte di LB
- CI: fmt, clippy, unit test, test datapath, dan trafik nyata lewat rig netns

## Berikutnya

**Benchmark yang bisa dipertanggungjawabkan.** Prioritas satu, dan satu-satunya item yang benar-benar
terhalang: butuh NIC fisik dengan dukungan XDP native. Semua pengujian sekarang berjalan di mode SKB
di atas veth, yang justru membuang keuntungan performa utama XDP. Angka dari mode SKB tidak boleh
dilaporkan sebagai angka XDP.

Yang dibutuhkan: pembanding nginx stream dan IPVS di mesin yang sama, tiga metrik (pps per core, p99
latency, CPU cycles per paket), dan generator trafik yang memakai port sumber acak.

**Penanganan MTU untuk DSR.** Enkapsulasi menambah 20 byte, dan paket yang sudah sebesar MTU akan
melewatinya setelah dibungkus lalu dibuang driver tanpa jejak. Yang dibutuhkan bukan pemeriksaan MTU
hardcoded di datapath — itu menanamkan asumsi angka — tapi MTU egress yang dibaca control plane dari
interface dan ditulis ke map, plus counter khusus untuk paket yang ditolak karenanya.

**CI multi-kernel.** CI sekarang menguji satu kernel saja, yaitu apa pun yang dipakai runner
`ubuntu-24.04`. Klaim kompatibilitas kernel 5.15+ di README belum ada buktinya. Butuh `vmtest` atau
`little-vm-helper` untuk menjalankan test yang sama di beberapa versi kernel.

**Distribusi yang terukur benar.** `test/smoke.sh` memakai `curl` berurutan, jadi port sumbernya
hampir berurutan dan sampelnya kecil. Rasio antar bobot yang berbeda terukur konsisten, tapi
pembagian pada bobot yang sama bisa terlihat miring. Butuh generator dengan port sumber acak dan
sampel jauh lebih besar sebelum ada klaim soal kualitas distribusi maglev.

**Conntrack yang bisa diinspeksi.** Isi map conntrack sekarang hanya bisa dilihat lewat `bpftool map
dump`. Endpoint yang menampilkan jumlah entry dan umur flow akan mempermudah debugging.

## Sengaja tidak dikerjakan

**Kubernetes operator.** Menarik, tapi mayoritas kodenya boilerplate reconcile yang tidak
menunjukkan apa pun soal eBPF. Lebih baik jadi project terpisah.

**IPv6.** Menggandakan jalur parsing dan conntrack tanpa menambah hal baru yang dipelajari.

**L7 routing.** XDP tidak melihat stream TCP, hanya paket individual. Routing berbasis HTTP header
butuh terminasi koneksi, yang berarti keluar dari XDP.

**Rate limiting yang presisi.** Bucket sekarang per-CPU dan karenanya aproksimatif. Membuatnya
presisi butuh state bersama, yang berarti cache line yang diperebutkan di jalur panas — menukar
alasan utama memakai XDP dengan ketepatan angka yang tidak diperlukan oleh mekanisme pertahanan.
