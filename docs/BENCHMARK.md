# Benchmark

## Apa yang diukur di sini, dan apa yang tidak

Yang diukur: **biaya CPU per paket di dalam program XDP**, lewat `BPF_PROG_TEST_RUN`. Kernel
menjalankan program terhadap paket sintetis dan melaporkan waktu yang dihabiskan di dalamnya. Tidak
ada NIC, tidak ada driver, tidak ada generator trafik yang berebut CPU.

Yang **tidak** diukur: throughput sistem. Angka di sini tidak bisa dibandingkan dengan angka pps
nginx atau IPVS, karena tidak menyertakan driver, alokasi buffer, penjadwalan softirq, maupun
kontensi. Untuk perbandingan semacam itu dibutuhkan dua mesin bare-metal, dan itu masih ada di
roadmap.

Kolom `Mpkt/s/core` adalah satu detik dibagi biaya per paket. Itu **batas atas** yang dipaksakan
datapath pada satu core, bukan angka yang akan kamu lihat di produksi.

```
make bench      # butuh root
```

## Kenapa metodenya tidak sesederhana yang terlihat

`BPF_PROG_TEST_RUN` punya parameter `repeat` yang menjalankan program ribuan kali dalam satu syscall
lalu melaporkan rata-rata. Terlihat sempurna untuk microbenchmark. Percobaan pertama memakainya, dan
hasilnya mustahil:

```
dsr, established flow                    TX       10.0         100.00
nat, established flow                    TX       30.0          33.33
```

DSR terukur tiga kali lebih cepat daripada NAT, padahal DSR menambah header IPv4 penuh dan menghitung
checksum dari nol sementara NAT hanya mengedit beberapa field. Dan 10 ns itu sama persis dengan biaya
jalur non-IP yang keluar di baris pertama fungsi.

Penyebabnya: **kernel tidak memulihkan isi paket antar repeat.** Diverifikasi, bukan diduga:

```
1000 in-kernel repeats of one nat flow produced miss=1 hit=0 pass=999
```

Hanya repeat pertama yang menempuh jalur NAT. Repeat berikutnya melihat paket yang destination-nya
sudah ditulis ulang ke backend, yang 5-tuple-nya tidak cocok dengan entry conntrack mana pun dan
tidak cocok dengan service mana pun, lalu keluar lewat `XDP_PASS`. Untuk DSR lebih parah: repeat
kedua melihat header luar IPIP, yang bukan TCP maupun UDP, jadi keluar di pemeriksaan protokol.

Jadi 999 dari 1000 sampel mengukur jalur yang salah. Perilaku ini sekarang dijaga test
(`prog_test_run_does_not_restore_the_packet_between_repeats`) supaya tidak menjebak orang lain.

**Metode yang dipakai sekarang:** setiap jalur yang memodifikasi paket diukur satu panggilan
sekaligus, masing-masing diberi salinan paket yang bersih. Konsekuensinya kernel menghitung waktu
untuk satu invokasi alih-alih mengamortisasi jam selama sejuta — jadi ada biaya tambahan per
panggilan. Biaya itu dikalibrasi pada jalur yang tidak menyentuh paket, tempat kedua metode
sama-sama valid, lalu dikurangkan:

```
non-ip frame, 1000000 in-kernel repeats : 11 ns/packet
non-ip frame, 20000 single calls        : 37 ns/packet
per-call overhead subtracted below      : 26 ns
```

Setelah dikurangkan, jalur non-IP kembali ke 11 ns — kalibrasinya konsisten dengan dirinya sendiri.

## Hasil

Semua angka median dari 5 run, dengan rentang min-maks di dalam tanda kurung. Median dipakai, bukan
rata-rata, karena satu lingkungan menghasilkan outlier ekstrem — penjelasannya di bawah tabel.

| Skenario | aarch64 (ns) | x86_64 (ns) |
| --- | --- | --- |
| non-ip frame, diteruskan | **11** (8–16) | **34** (22–64) |
| destinasi tak dikenal, diteruskan | **35** (29–37) | **99** (88–137) |
| nat, flow berjalan | **68** (61–72) | **160** (150–193) |
| dsr, flow berjalan | **68** (64–73) | **169** (150–1713) |
| rate limited, di-drop | **215** (195–303) | **336** (301–429) |
| tanpa backend, di-drop | **220** (207–254) | **322** (278–421) |
| dsr, flow baru | **284** (245–329) | **509** (457–1997) |
| nat, flow baru | **413** (392–514) | **734** (714–2220) |

**aarch64** — kernel 6.8.0-137, 4 core, Lima VM di Apple Silicon. `steal` nol sebelum dan sesudah
pengujian. Rentangnya rapat: maks/min sekitar 1,3–1,6×.

**x86_64** — kernel 6.8.0-49, 1 vCPU, Intel Xeon Platinum 8176 @ 2.10 GHz, VPS. **Lingkungan ini
tidak layak untuk mengukur performa** dan angkanya hanya indikatif. `steal` naik 21772 → 25818 tick
selama pengujian, sekitar 40 detik CPU yang dirampas hypervisor. Akibatnya tiap run punya satu
outlier 4–12× di posisi yang berbeda-beda:

```
run 1: dsr flow baru    = 1997 ns   (median 509)
run 2: dsr flow berjalan = 1713 ns  (median 169)
run 3: nat flow baru    = 2220 ns   (median 734)
```

Nilai mediannya tetap konsisten, jadi tabel di atas masih berguna untuk melihat **urutan dan rasio**
antar skenario. Nilai absolutnya dari kolom x86_64 sebaiknya tidak dikutip.

## Yang bisa dibaca dari angka ini

**Flow berjalan jauh lebih murah daripada flow baru.** Di aarch64, 68 ns lawan 413 ns untuk NAT —
enam kali. Di x86_64, 160 lawan 734 — 4,6 kali. Ini alasan conntrack ada. Dari pengukuran trafik
nyata sebelumnya, 440 dari 480 paket adalah conntrack hit, jadi jalur murah itu yang mendominasi
beban sesungguhnya.

**DSR memotong 31% biaya pembentukan flow baru, dan angkanya identik di dua arsitektur.**

| | aarch64 | x86_64 |
| --- | --- | --- |
| nat, flow baru | 413 ns | 734 ns |
| dsr, flow baru | 284 ns | 509 ns |
| selisih | **−31%** | **−31%** |

Bahwa dua CPU yang sangat berbeda menghasilkan persentase yang sama membuat ini bukan kebetulan
pengukuran. Dan penyebabnya bisa ditunjuk di kode: NAT menulis **dua** entry conntrack, satu untuk
tiap arah, sementara DSR hanya menulis **satu** karena balasan tidak akan pernah lewat load balancer.
Selisihnya adalah harga satu insert ke LRU hash.

**Untuk flow yang sudah berjalan, DSR dan NAT tidak terbedakan.** Keduanya 68 ns di aarch64; di
x86_64 selisihnya 160 lawan 169 ns, di dalam noise lingkungan itu. Pengukuran satu run sebelumnya
sempat menunjukkan DSR lebih mahal, tapi itu tidak bertahan di median lima run. Secara intuitif DSR
seharusnya lebih mahal — ia menambah header dan menghitung checksum penuh — tapi metode ini tidak
punya resolusi untuk membuktikannya.

Keunggulan DSR yang sebenarnya juga bukan di sini, melainkan di **jumlah paket yang harus diproses
sama sekali**: 42% lebih sedikit paket dan 47% lebih sedikit byte untuk beban kerja yang sama. Lihat
`docs/FORWARDING.md`.

**Jalur PASS murah** — 11 ns untuk frame non-IP, 35 ns setelah parsing penuh. Ini penting karena
seluruh trafik yang tidak ditujukan ke VIP mana pun tetap harus melewati program ini. Biaya yang
dibebankan pada trafik yang tidak berkepentingan mendekati nol, dan itu prasyarat untuk memasang XDP
di interface produksi tanpa merugikan trafik lain.

**Rate limiting tidak gratis** — 215 ns, sekitar tiga kali biaya jalur flow berjalan. Selisihnya
lookup dan update ke LRU per-CPU hash. Wajar untuk mekanisme pertahanan yang hanya aktif saat
pembentukan flow baru, tapi cukup mahal untuk menjelaskan kenapa defaultnya mati.

## Batasan yang jujur

- **Tidak ada bare-metal.** Keduanya VM. Biaya absolutnya akan berbeda di perangkat lain; yang bisa
  dipegang adalah rasio antar skenario, yang terbukti konsisten di dua arsitektur.
- **Nilai di bawah ~35 ns ada di batas resolusi metode ini.** Rentang 8–16 ns untuk jalur non-IP
  berarti selisih beberapa nanosekon tidak bisa dibedakan dari noise.
- **Kalibrasi mengasumsikan biaya tambahan per panggilan konstan** antar skenario. Wajar karena biaya
  itu ada di kernel di luar program, tapi tetap sebuah asumsi.
- **Angka sensitif terhadap lokalitas key.** Skenario drop awalnya diukur dengan paket yang sama
  berulang, sehingga lookup conntrack-nya selalu cache-hot, dan hasilnya 86 ns. Dengan port sumber
  yang bervariasi seperti flow baru sungguhan, angkanya 220 ns. Yang kedua yang dipakai.
- **Belum ada perbandingan dengan nginx atau IPVS.** Itu pekerjaan berbeda: butuh dua mesin
  bare-metal, satu untuk membangkitkan trafik dan satu untuk diuji.
